package com.anyboot.remote;

import com.jcraft.jsch.ChannelShell;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * 一个终端会话：SSH shell 通道或 Telnet socket 的统一外壳。
 *
 * <p>双向字节流：
 * <ul>
 *   <li>下行（远端 → 界面）：读取线程收字节 → {@link Listener#onData}；
 *       未挂到界面时先攒进 {@link #backlog}（封顶 256KB，够滚动回看），挂上后一次喂给 xterm.js；</li>
 *   <li>上行（键盘 → 远端）：{@link #write(byte[])}。</li>
 * </ul>
 *
 * <p>会话由 {@link TerminalRegistry} 持有，Activity 销毁不关会话 —— 切后台 / 切页面
 * 终端照常活着，回到终端页接着用。
 */
final class TerminalSession {

    interface Listener {
        /** 远端输出（界面线程由注册表保证切好） */
        void onData(byte[] bytes);

        /** 会话结束（远端关闭 / 网络断开 / 用户主动结束） */
        void onClosed(String reason);
    }

    /** 在哪个服务器上 */
    final JumpServer server;
    final long startedAt = System.currentTimeMillis();

    /** 可更换：会话比 Activity 活得久，界面重建后要重挂到活着的那个上 */
    private volatile Listener listener;
    private ChannelShell shell;
    private TelnetSession telnet;
    private Thread reader;
    private volatile boolean closed = false;
    private volatile boolean attached = false;
    private final ByteArrayOutputStream backlog = new ByteArrayOutputStream();
    private static final int BACKLOG_CAP = 256 * 1024;
    private String closeReason = "";

    /** 数据锁：backlog 与 listener 派发共用一把，防下行乱序 */
    private final Object dataLock = new Object();

    TerminalSession(JumpServer server, Listener listener) {
        this.server = server;
        this.listener = listener;
    }

    // ------------------------------------------------------------------
    // 建立（后台线程）
    // ------------------------------------------------------------------

    /** SSH：先确保会话，再开 shell 通道 */
    void startSsh(android.content.Context context, SshSessionManager.Prompt prompt) throws Exception {
        shell = SshSessionManager.shared().openShell(context, server, prompt);
        final InputStream input = shell.getInputStream();
        final OutputStream output = shell.getOutputStream();
        this.output = output;
        reader = new Thread(() -> pump(input, "连接已关闭"), "anyboot-shell");
        reader.setDaemon(true);
        reader.start();
    }

    /** Telnet：socket 直连（实验性） */
    void startTelnet() throws Exception {
        telnet = new TelnetSession();
        telnet.start(server, new TelnetSession.Listener() {
            @Override
            public void onData(byte[] bytes) {
                deliver(bytes);
            }

            @Override
            public void onClosed(String reason) {
                close(reason);
            }
        });
        this.output = telnetOutput();
    }

    private OutputStream telnetOutput() {
        return new OutputStream() {
            @Override
            public void write(int b) {
                telnet.write(new byte[] {(byte) b});
            }

            @Override
            public void write(byte[] b) {
                telnet.write(b);
            }

            @Override
            public void write(byte[] b, int off, int len) {
                byte[] slice = new byte[len];
                System.arraycopy(b, off, slice, 0, len);
                telnet.write(slice);
            }

            @Override
            public void flush() {
                // TelnetSession.write 里已经 flush
            }
        };
    }

    // ------------------------------------------------------------------
    // 数据
    // ------------------------------------------------------------------

    private OutputStream output;

    /** 键盘 → 远端 */
    void write(byte[] bytes) {
        OutputStream out = output;
        if (out == null || closed) {
            return;
        }
        try {
            synchronized (dataLock) {
                out.write(bytes);
                out.flush();
            }
        } catch (Exception ignored) {
            close("发送失败");
        }
    }

    /** 重挂监听器（bind 时调用）：否则回显仍喂给已销毁的旧 Activity，输入的字有去无回 */
    void setListener(Listener replacement) {
        synchronized (dataLock) {
            this.listener = replacement;
        }
    }

    /** 挂上界面并取走积攒的回显（切回终端页 / 切会话标签都走这里） */
    byte[] attachAndDrain() {
        synchronized (dataLock) {
            attached = true;
            byte[] data = backlog.toByteArray();
            backlog.reset();
            return data;
        }
    }

    void detach() {
        synchronized (dataLock) {
            attached = false;
        }
    }

    /** 终端窗口变了（xterm.js fit 之后调）：SSH 侧会触发远端 SIGWINCH */
    void resize(int columns, int rows) {
        if (shell != null && shell.isConnected()) {
            try {
                shell.setPtySize(columns, rows, 0, 0);
            } catch (Exception ignored) {
                // resize 失败不影响终端继续用
            }
        }
    }

    boolean isClosed() {
        return closed;
    }

    String closeReason() {
        return closeReason;
    }

    void close(String reason) {
        if (closed) {
            return;
        }
        closed = true;
        closeReason = reason == null ? "" : reason;
        try {
            if (shell != null) {
                shell.disconnect();
            }
        } catch (Exception ignored) {
            // 关就关了
        }
        try {
            if (telnet != null) {
                telnet.close(null);
            }
        } catch (Exception ignored) {
            // 同上
        }
        synchronized (dataLock) {
            attached = false;
        }
        listener.onClosed(closeReason);
    }

    /** 读线程：远端 → 派发（关闭回调统一在 {@link #close} 里发，别发两次） */
    private void pump(InputStream input, String endReason) {
        byte[] chunk = new byte[4096];
        int count;
        try {
            while (!closed && (count = input.read(chunk)) > 0) {
                byte[] slice = new byte[count];
                System.arraycopy(chunk, 0, slice, 0, count);
                deliver(slice);
            }
            close(endReason);
        } catch (Exception error) {
            if (!closed) {
                close("连接断开：" + error.getMessage());
            }
        }
    }

    private void deliver(byte[] bytes) {
        synchronized (dataLock) {
            if (!attached && backlog.size() < BACKLOG_CAP) {
                backlog.write(bytes, 0, bytes.length);
            }   // 只攒「未挂界面」期间的；挂着时 backlog 不再积累，attachAndDrain 才不会把历史重印一遍
            if (attached) {
                listener.onData(bytes);
            }
        }
    }
}
