package com.anyboot.remote;

import android.content.Context;

import com.jcraft.jsch.ChannelShell;
import com.jcraft.jsch.HostKey;
import com.jcraft.jsch.HostKeyRepository;
import com.jcraft.jsch.JSch;
import com.jcraft.jsch.Session;

import java.util.HashMap;
import java.util.Map;

/**
 * SSH 会话栈（全部服务器功能的底座）：一个跳板机一条 {@link Session}，多个用途复用。
 *
 * <ul>
 *   <li><b>隧道</b> {@link #openTunnel}：direct-tcpip 本地转发 —— WebView / 心跳 / API
 *       全部走 {@code 127.0.0.1:随机端口}；</li>
 *   <li><b>终端</b> {@link #openShell}：shell 通道，字节流交给 xterm.js；</li>
 *   <li><b>SFTP</b>：文件页拿 {@link #session} 自己开 {@code sftp} subsystem。</li>
 * </ul>
 *
 * <p>要点：
 * <ul>
 *   <li>断线重连带指数退避（1s → 2s → 4s … 封顶 60s），防止弱网下疯狂重试；</li>
 *   <li>主机指纹 TOFU：首次连接弹窗确认，存住；对不上（中间人）直接拒连；</li>
 *   <li>密码 / 私钥从 {@link KeyVault} 解密，不落明文日志。</li>
 * </ul>
 */
final class SshSessionManager {

    /** 主线程模态确认（TOFU 指纹弹窗用）；实现必须自己切主线程并同步等待用户 */
    interface Prompt {
        boolean confirm(String title, String message);
    }

    private static final SshSessionManager SHARED = new SshSessionManager();

    static SshSessionManager shared() {
        return SHARED;
    }

    private SshSessionManager() {
    }

    /** 指数退避：第 n 次连续失败后，n 秒内不许再连（封顶 60s） */
    private static final long BACKOFF_CAP_MS = 60000;

    private static final class Entry {
        Session session;
        int consecutiveFails = 0;
        long nextAttemptAt = 0;
        /** jumpId|destHost|destPort → 本地转发端口（复用） */
        final Map<String, Integer> tunnels = new HashMap<>();
        /** 经由跳板机连接时：跳板会话 + 在它上面占的那条转发端口（断开时撤） */
        Session viaSession;
        int viaForwardPort = -1;
    }

    private final Map<String, Entry> entries = new HashMap<>();

    // ------------------------------------------------------------------
    // 会话
    // ------------------------------------------------------------------

    /** 取已连接的会话；没有给 null（不连接） */
    Session liveSession(String jumpId) {
        Entry entry = entries.get(jumpId);
        if (entry == null || entry.session == null) {
            return null;
        }
        if (!entry.session.isConnected()) {
            invalidate(jumpId);
            return null;
        }
        return entry.session;
    }

    boolean isUp(String jumpId) {
        return liveSession(jumpId) != null;
    }

    /** 连接出问题后调用：立刻断掉，退避计时走起 */
    void invalidate(String jumpId) {
        Entry entry = entries.get(jumpId);
        if (entry == null) {
            return;
        }
        if (entry.session != null) {
            try {
                entry.session.disconnect();
            } catch (Exception ignored) {
                // 断都断了
            }
            entry.session = null;
        }
        releaseVia(entry);
        entry.tunnels.clear();
        entry.consecutiveFails++;
        long delay = Math.min(BACKOFF_CAP_MS, 1000L << Math.min(entry.consecutiveFails, 6));
        entry.nextAttemptAt = System.currentTimeMillis() + delay;
    }

    /** 主动关闭（删除服务器时） */
    void shutdown(String jumpId) {
        Entry entry = entries.remove(jumpId);
        if (entry != null) {
            if (entry.session != null) {
                try {
                    entry.session.disconnect();
                } catch (Exception ignored) {
                    // 同上
                }
            }
            releaseVia(entry);
        }
    }

    /** 经由跳板机连接时，撤掉在跳板会话上占的那条转发（会话可能已死，异常忽略） */
    private void releaseVia(Entry entry) {
        if (entry.viaSession != null && entry.viaForwardPort > 0) {
            try {
                entry.viaSession.delPortForwardingL("127.0.0.1", entry.viaForwardPort);
            } catch (Exception ignored) {
                // 跳板会话都没了，转发自然跟着没了
            }
        }
        entry.viaSession = null;
        entry.viaForwardPort = -1;
    }

    /** 编辑保存后清退避状态（不动在线会话）—— 改完密码能立刻重连 */
    void resetBackoff(String jumpId) {
        Entry entry = entries.get(jumpId);
        if (entry != null) {
            entry.consecutiveFails = 0;
            entry.nextAttemptAt = 0;
        }
    }

    // ------------------------------------------------------------------
    // 手动输密码应急通道（不落盘、不碰 Keystore）
    // ------------------------------------------------------------------

    private final java.util.concurrent.ConcurrentHashMap<String, String> manualPasswords =
            new java.util.concurrent.ConcurrentHashMap<>();

    /** 密码认证但本机没存到密码（Keystore 失败/未保存）→ 连接前需要用户手输一次 */
    boolean needsManualPassword(JumpServer server) {
        boolean typed = manualPasswords.containsKey(server.id);
        String blob = JumpStore.text(server.passwordEnc);
        boolean decryptable = !KeyVault.decrypt(blob).isEmpty();
        boolean needs = JumpServer.AUTH_PASSWORD.equals(server.authType)
                && !typed
                && !decryptable;
        // 密文长度非 0 却解不开 = 密钥库那把密钥被换过（每次调 key() 都会覆盖的历史遗留）
        TerminalActivity.tlog("needsManualPassword 认证=" + server.authType
                + " 手输过=" + typed + " 密文=" + blob.length() + "B 可解=" + decryptable
                + " -> " + needs);
        return needs;
    }

    /** 记住本次运行手动输入的密码（只放内存）；传空串清除 */
    void setManualPassword(String jumpId, String password) {
        if (password == null || password.isEmpty()) {
            manualPasswords.remove(jumpId);
        } else {
            manualPasswords.put(jumpId, password);
        }
    }

    /**
     * 确保有一条已连接的会话（复用或新建）。**必须在后台线程调用**（连接是阻塞的）。
     *
     * @throws Exception 连接失败 / 退避期内 / 用户拒绝了主机指纹
     */
    Session ensureSession(Context context, JumpServer server, Prompt prompt) throws Exception {
        return ensureSession(context, server, prompt, true);
    }

    /**
     * 带链路开关的内部版。{@code allowChain=false} 用在「作为跳板机被别人连」的
     * 递归里：单层链，跳板自己的 jumpServerId 不再跟（防自引 / 成环）。
     */
    private Session ensureSession(Context context, JumpServer server, Prompt prompt,
                                  boolean allowChain) throws Exception {
        Session live = liveSession(server.id);
        if (live != null) {
            return live;
        }
        Entry entry = entries.get(server.id);
        if (entry == null) {
            entry = new Entry();
            entries.put(server.id, entry);
        }
        long now = System.currentTimeMillis();
        if (now < entry.nextAttemptAt) {
            long wait = (entry.nextAttemptAt - now) / 1000 + 1;
            throw new Exception("连接失败过，" + wait + " 秒后再试（自动退避）");
        }
        try {
            JSch jsch = new JSch();
            if (JumpServer.AUTH_KEY.equals(server.authType)) {
                SshKeyEntry key = JumpStore.findKey(JumpStore.loadKeys(context), server.keyId);
                if (key == null) {
                    throw new Exception("找不到选中的密钥，请到「密钥」页检查");
                }
                String pem = KeyVault.decrypt(key.privateEnc);
                if (pem.isEmpty()) {
                    throw new Exception("私钥解密失败（换机恢复后需重新导入）");
                }
                String passphrase = KeyVault.decrypt(server.keyPassphraseEnc);
                jsch.addIdentity("anyboot-" + server.id,
                        pem.getBytes("UTF-8"),
                        key.publicKey == null ? null : key.publicKey.getBytes("UTF-8"),
                        passphrase.getBytes("UTF-8"));
            }
            // —— 经由跳板机：先确保跳板在线，再在跳板会话上开一条自动端口的
            //    本地转发指向目标 ssh 端口，目标会话改连 127.0.0.1:转发口 ——
            String connectHost = server.hostname();
            int connectPort = server.port();
            if (allowChain && !JumpStore.text(server.jumpServerId).isEmpty()) {
                if (!server.isSsh()) {
                    throw new Exception("Telnet 服务器不支持经由跳板机连接（隧道只走 SSH）");
                }
                JumpServer jump = JumpStore.findJump(JumpStore.loadJumps(context), server.jumpServerId);
                if (jump == null) {
                    throw new Exception("找不到经由的跳板机，请到「服务器」页检查，或改回直连");
                }
                if (!jump.isSsh()) {
                    throw new Exception("跳板机只支持 SSH：「" + JumpStore.text(jump.name) + "」是 Telnet");
                }
                Session viaSession = ensureSession(context, jump, prompt, false);
                connectPort = viaSession.setPortForwardingL("127.0.0.1", 0, connectHost, connectPort);
                connectHost = "127.0.0.1";
                entry.viaSession = viaSession;
                entry.viaForwardPort = connectPort;
            }
            Session session = jsch.getSession(
                    JumpStore.text(server.username).isEmpty() ? "root" : server.username,
                    connectHost, connectPort);
            if (entry.viaSession != null) {
                // 指纹按真实主机记/查（TOFU 存储以 server.host 为键），别记成 127.0.0.1
                session.setHostKeyAlias(server.host);
            }
            if (JumpServer.AUTH_PASSWORD.equals(server.authType)) {
                // 优先用手动输入的应急密码，其次读落盘的
                String password = manualPasswords.containsKey(server.id)
                        ? manualPasswords.get(server.id)
                        : KeyVault.decrypt(server.passwordEnc);
                if (password.isEmpty()) {
                    throw new Exception("密码未保存或解密失败 —— 请点「编辑」重新输入一次密码再连");
                }
                session.setPassword(password);
                // 部分服务器（PAM）不吃纯 password，要用 keyboard-interactive 应答同一密码
                session.setUserInfo(new PasswordUserInfo(password));
            }
            session.setHostKeyRepository(new TofuRepository(context, server, prompt));
            session.setConfig("StrictHostKeyChecking", "yes");
            session.setConfig("PreferredAuthentications",
                    JumpServer.AUTH_KEY.equals(server.authType)
                            ? "publickey,keyboard-interactive,password"
                            : "password,keyboard-interactive,publickey");
            session.setTimeout(30000);           // socket 读超时（隧道空闲时不影响）
            session.setServerAliveInterval(15000); // 15s 心跳包，NAT 保活
            session.setServerAliveCountMax(3);
            session.connect(15000);
            entry.session = session;
            entry.consecutiveFails = 0;
            entry.nextAttemptAt = 0;
            return session;
        } catch (com.jcraft.jsch.JSchException error) {
            releaseVia(entry);   // 连接没成，跳板上占的转发一并撤掉
            String message = String.valueOf(error.getMessage());
            if (message.startsWith("Auth fail")) {
                throw new Exception("认证失败：用户名或密码不正确（服务器拒绝了登录）", error);
            }
            if (message.contains("Auth cancel")) {
                throw new Exception("认证被取消", error);
            }
            throw error;
        } catch (Exception error) {
            releaseVia(entry);   // 同上
            entry.consecutiveFails++;
            long delay = Math.min(BACKOFF_CAP_MS, 1000L << Math.min(entry.consecutiveFails, 6));
            entry.nextAttemptAt = System.currentTimeMillis() + delay;
            throw error;
        }
    }

    // ------------------------------------------------------------------
    // 隧道（本地端口转发）
    // ------------------------------------------------------------------

    /**
     * 打开（或复用）一条到 destHost:destPort 的本地转发。
     *
     * @return 本地端口（监听在 127.0.0.1，不暴露）。**后台线程调用。**
     */
    int openTunnel(Context context, JumpServer server, String destHost, int destPort,
                   Prompt prompt) throws Exception {
        Session session = ensureSession(context, server, prompt);
        Entry entry = entries.get(server.id);
        String tunnelKey = destHost + ":" + destPort;
        Integer existing = entry.tunnels.get(tunnelKey);
        if (existing != null) {
            try {
                int rebound = session.setPortForwardingL("127.0.0.1", existing, destHost, destPort);
                entry.tunnels.put(tunnelKey, rebound);
                return rebound;
            } catch (Exception alreadyBound) {
                return existing;   // 还在监听 = 复用
            }
        }
        int localPort = session.setPortForwardingL("127.0.0.1", 0, destHost, destPort);
        entry.tunnels.put(tunnelKey, localPort);
        return localPort;
    }

    /** 经由跳板机到 dest 的本地地址（连接前先解析）；失败抛异常 */
    String tunnelAddress(Context context, JumpServer server, String destHostPort,
                         Prompt prompt) throws Exception {
        String host = destHostPort;
        int port = 12300;
        int colon = destHostPort.lastIndexOf(':');
        if (colon > 0) {
            host = destHostPort.substring(0, colon);
            try {
                port = Integer.parseInt(destHostPort.substring(colon + 1));
            } catch (NumberFormatException ignored) {
                // 保持默认端口
            }
        }
        int localPort = openTunnel(context, server, host, port, prompt);
        return "127.0.0.1:" + localPort;
    }

    // ------------------------------------------------------------------
    // 终端
    // ------------------------------------------------------------------

    /**
     * 密码认证的 UserInfo：除了 getPassword，还实现
     * {@link com.jcraft.jsch.UIKeyboardInteractive} —— PAM 服务器把
     * password 协商成 keyboard-interactive 时，用同一密码应答质询，
     * 不再弹框打断用户。
     */
    private static final class PasswordUserInfo
            implements com.jcraft.jsch.UserInfo, com.jcraft.jsch.UIKeyboardInteractive {
        private final String password;

        PasswordUserInfo(String password) {
            this.password = password;
        }

        @Override
        public String getPassword() {
            return password;
        }

        @Override
        public boolean promptPassword(String message) {
            return true;   // 密码已持有，让 JSch 直接取
        }

        @Override
        public String getPassphrase() {
            return null;   // 口令认证走密钥的 addIdentity，不经这里
        }

        @Override
        public boolean promptPassphrase(String message) {
            return true;
        }

        @Override
        public boolean promptYesNo(String message) {
            return false;  // 主机指纹走 TofuRepository，这里不代答确认框
        }

        @Override
        public void showMessage(String message) {
            // 忽略服务器 banner 提示
        }

        @Override
        public String[] promptKeyboardInteractive(String destination, String name,
                String instruction, String[] prompt, boolean[] echo) {
            if (prompt == null || prompt.length == 0) {
                return new String[0];
            }
            // 常见就一个「Password:」质询；多个质询时其余置空
            String[] answers = new String[prompt.length];
            for (int i = 0; i < prompt.length; i++) {
                answers[i] = i == 0 ? password : "";
            }
            return answers;
        }
    }

    /** 打开 shell 通道（终端用）。**后台线程调用。** */
    ChannelShell openShell(Context context, JumpServer server, Prompt prompt) throws Exception {
        Session session = ensureSession(context, server, prompt);
        ChannelShell channel = (ChannelShell) session.openChannel("shell");
        // xterm.js 全权负责回显 —— shell 端关掉回显请求，避免输入重影
        channel.setPtyType("xterm-256color", 80, 24, 0, 0);
        channel.connect(15000);
        // 启动路径 / 默认 shell（与 PC 端 hub_term.rs 同一套语义）：
        // 往登录 shell 的 stdin 写一行命令，像手敲一样执行。目录打不通时
        // 报错留在终端里，`;` 继续往下走，绝不因填错路径断会话。
        String startup = terminalStartupLine(server);
        if (!startup.isEmpty()) {
            java.io.OutputStream stdin = channel.getOutputStream();
            stdin.write(startup.getBytes("UTF-8"));
            stdin.write('\n');
            stdin.flush();
        }
        return channel;
    }

    /**
     * 组终端启动行（PC 端 term_remote_command 的 ChannelShell 版）：
     * <ul>
     *   <li>路径与 shell 都空 → 空串（什么都不注入，纯登录）；</li>
     *   <li>只配路径 → {@code cd '<path>'}（shell 通道本来就是交互登录 shell，不用 exec）；</li>
     *   <li>配了 shell → {@code exec <shell>}。预设（bash/sh/zsh/fish）带 {@code -l}
     *       对齐 login-shell 语义并单引号包住；自定义内容原样 exec（tmux 之类自己写完整命令）。</li>
     * </ul>
     */
    static String terminalStartupLine(JumpServer server) {
        String path = server.termPath == null ? "" : server.termPath.trim();
        String shell = server.termShell == null ? "" : server.termShell.trim();
        StringBuilder line = new StringBuilder();
        if (!path.isEmpty()) {
            line.append("cd ").append(shQuote(path));
        }
        if (!shell.isEmpty()) {
            if (line.length() > 0) {
                line.append("; ");
            }
            switch (shell) {
                case "/bin/bash":
                case "/bin/sh":
                case "/bin/zsh":
                case "/bin/fish":
                    line.append("exec ").append(shQuote(shell)).append(" -l");
                    break;
                default:
                    line.append("exec ").append(shell);
                    break;
            }
        }
        return line.toString();
    }

    /** 远程命令里的安全引号：单引号包住，内嵌 {@code '} 按 {@code '\''} 断开再接。 */
    private static String shQuote(String value) {
        return "'" + value.replace("'", "'\\''") + "'";
    }

    // ------------------------------------------------------------------
    // 主机指纹 TOFU
    // ------------------------------------------------------------------

    /**
     * TOFU（Trust On First Use）：首连弹窗确认并记住；之后对得上就放行，
     * 对不上（中间人 / 重装系统）直接拒绝 —— 用户可在服务器详情里「重置信任」。
     *
     * <p>mwiede JSch 0.2.x 里 {@link HostKeyRepository} 是 interface，
     * check 收到的是 {@code (host, keyBlob)} —— 指纹自己按 OpenSSH 口径
     * 算（SHA256(blob)），别用 HostKey.getFingerPrint（那是 MD5 口径）。
     */
    private final class TofuRepository implements HostKeyRepository {
        private final Context context;
        private final JumpServer server;
        private final Prompt prompt;

        TofuRepository(Context context, JumpServer server, Prompt prompt) {
            this.context = context.getApplicationContext();
            this.server = server;
            this.prompt = prompt;
        }

        @Override
        public int check(String host, byte[] key) {
            String stored = JumpStore.trustedHostKey(context, server.host);
            String finger = key == null ? "" : KeyVault.sha256Fingerprint(key);
            if (stored.isEmpty()) {
                // 首连：弹窗确认（同步等用户）
                String message = "首次连接 " + server.addressLabel()
                        + "\n\n主机指纹（SHA256）：" + finger
                        + "\n\n确认是你要连的服务器吗？（确认后记住，不再询问）";
                boolean accepted = prompt != null && prompt.confirm("确认主机指纹", message);
                if (!accepted) {
                    return NOT_INCLUDED;
                }
                JumpStore.setTrustedHostKey(context, server.host, finger);
                return OK;
            }
            return stored.equals(finger) ? OK : NOT_INCLUDED;
        }

        @Override
        public void add(HostKey hostkey, com.jcraft.jsch.UserInfo info) {
            // TOFU 的记忆在 check 里写，这里不用
        }

        @Override
        public void remove(String host, String type) {
            JumpStore.setTrustedHostKey(context, server.host, "");
        }

        @Override
        public void remove(String host, String type, byte[] key) {
            remove(host, type);
        }

        @Override
        public String getKnownHostsRepositoryID() {
            return "anyboot-tofu";
        }

        @Override
        public HostKey[] getHostKey() {
            return new HostKey[0];
        }

        @Override
        public HostKey[] getHostKey(String host, String type) {
            return new HostKey[0];
        }
    }
}
