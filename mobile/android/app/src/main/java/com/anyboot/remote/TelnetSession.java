package com.anyboot.remote;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;

/**
 * Telnet 会话（实验性）：TCP socket 直连 + IAC 控制字节过滤。
 *
 * <p>Telnet 协议本身没有端口转发语义，只能给终端用（把远端回显喂给 xterm.js）。
 * 协商策略是「极简拒绝」：ECHO / SUPPRESS-GO-AHEAD 接受（终端体验需要），
 * 其余 WILL/DO 一律拒绝 —— 大多数 BBS / 老设备能正常出字。
 */
final class TelnetSession {

    private static final int IAC = 255;
    private static final int DONT = 254;
    private static final int DO = 253;
    private static final int WONT = 252;
    private static final int WILL = 251;

    interface Listener {
        /** 远端数据（已剥掉 IAC 控制字节）；在读取线程回调 */
        void onData(byte[] bytes);

        /** 连接关闭 / 出错（在读取线程回调） */
        void onClosed(String reason);
    }

    private Socket socket;
    private OutputStream output;
    private Thread reader;
    private volatile boolean closed = false;

    /** 建立连接并开始读。**后台线程调用**（连接阻塞）。 */
    void start(JumpServer server, Listener listener) throws Exception {
        socket = new Socket();
        socket.connect(new InetSocketAddress(server.hostname(), server.port()), 10000);
        socket.setTcpNoDelay(true);
        output = socket.getOutputStream();
        closed = false;
        reader = new Thread(() -> pump(listener), "anyboot-telnet");
        reader.setDaemon(true);
        reader.start();
    }

    /** 发数据到远端 */
    void write(byte[] bytes) {
        OutputStream out = output;
        if (out == null || closed) {
            return;
        }
        try {
            synchronized (this) {
                out.write(bytes);
                out.flush();
            }
        } catch (Exception ignored) {
            close("发送失败");
        }
    }

    void close(String reason) {
        if (closed) {
            return;
        }
        closed = true;
        try {
            if (socket != null) {
                socket.close();
            }
        } catch (Exception ignored) {
            // 关就关了
        }
    }

    boolean isAlive() {
        return !closed && socket != null && socket.isConnected() && !socket.isClosed();
    }

    // ------------------------------------------------------------------
    // 内部
    // ------------------------------------------------------------------

    private void pump(Listener listener) {
        try {
            InputStream input = socket.getInputStream();
            byte[] chunk = new byte[4096];
            ByteArrayOutputStream filtered = new ByteArrayOutputStream();
            int count;
            while (!closed && (count = input.read(chunk)) > 0) {
                filterIac(chunk, count, filtered, listener);
                if (filtered.size() > 0) {
                    listener.onData(filtered.toByteArray());
                    filtered.reset();
                }
            }
            close(null);
            listener.onClosed("连接已关闭");
        } catch (Exception error) {
            if (!closed) {
                close(null);
                listener.onClosed("连接断开：" + error.getMessage());
            }
        }
    }

    /** IAC 状态机：剥协商字节，必须应答的当场写回 socket */
    private void filterIac(byte[] chunk, int count, ByteArrayOutputStream out,
                           Listener listener) {
        int i = 0;
        while (i < count) {
            int b = chunk[i] & 0xFF;
            if (b == IAC) {
                if (i + 1 >= count) {
                    break;   // 半截命令等下一包
                }
                int cmd = chunk[i + 1] & 0xFF;
                if (cmd == IAC) {
                    out.write(0xFF);   // 转义：255.255 = 字面 255
                    i += 2;
                    continue;
                }
                if (cmd == WILL || cmd == DO || cmd == WONT || cmd == DONT) {
                    if (i + 2 >= count) {
                        break;
                    }
                    int option = chunk[i + 2] & 0xFF;
                    // 只接受 ECHO(1) / SGA(3)（WILL），其余一律拒绝；
                    // 对端要求我们 DO 什么，都 WONT —— 极简终端不实现选项
                    if (cmd == WILL && (option == 1 || option == 3)) {
                        reply(new byte[] {(byte) IAC, (byte) DO, (byte) option});
                    } else if (cmd == WILL) {
                        reply(new byte[] {(byte) IAC, (byte) DONT, (byte) option});
                    } else if (cmd == DO) {
                        reply(new byte[] {(byte) IAC, (byte) WONT, (byte) option});
                    }
                    i += 3;
                    continue;
                }
                // SB(250)…SE(240) 子协商 / 其它命令：整段丢弃（没实现终端类型协商）
                if (cmd == 250) {
                    i += 2;
                    while (i < count && (chunk[i] & 0xFF) != 240) {
                        i++;
                    }
                    i++;
                } else {
                    i += 2;
                }
                continue;
            }
            out.write(b);
            i++;
        }
    }

    private void reply(byte[] bytes) {
        try {
            synchronized (this) {
                output.write(bytes);
                output.flush();
            }
        } catch (Exception ignored) {
            // 对端不收就算了
        }
    }
}
