package com.anyboot.remote;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.UUID;

/**
 * 一台「服务器」= SSH / Telnet 跳板机（区别于「电脑」= anyboot 服务端）。
 *
 * <p>用途：外网连家里 —— 手机先 SSH 登录公网可达的跳板机，再由跳板机在局域网内
 * 访问 anyboot 电脑端 HTTP 接口（SSH direct-tcpip 本地端口转发）。
 *
 * <p>协议与能力：
 * <ul>
 *   <li><b>SSH</b>：隧道（端口转发）+ 交互终端 + SFTP，全部能力可用；</li>
 *   <li><b>Telnet</b>：只有交互终端（实验性，自动登录 + IAC 过滤），无隧道无文件。</li>
 * </ul>
 *
 * <p>认证：密码 / 密钥（密钥引用 {@link SshKeyEntry} 的 id）。密码与私钥
 * 都经 {@link KeyVault}（Android Keystore AES-GCM）加密后才落盘。
 */
final class JumpServer {

    static final String PROTO_SSH = "ssh";
    static final String PROTO_TELNET = "telnet";

    static final String AUTH_PASSWORD = "password";
    static final String AUTH_KEY = "key";

    static final int DEFAULT_SSH_PORT = 22;
    static final int DEFAULT_TELNET_PORT = 23;

    String id;
    String name = "";
    /** {@link #PROTO_SSH} / {@link #PROTO_TELNET} */
    String protocol = PROTO_SSH;
    /** host 或 host:port（不带协议头） */
    String host = "";
    String username = "";
    /** {@link #AUTH_PASSWORD} / {@link #AUTH_KEY} */
    String authType = AUTH_PASSWORD;
    /** 密码明文经 KeyVault 加密后的密文（base64）；密码认证时有值 */
    String passwordEnc = "";
    /** 引用 SshKeyEntry.id（密钥认证时）；密钥本体在密钥库里 */
    String keyId = "";
    /** 密钥口令（可空），同样 KeyVault 加密落盘 */
    String keyPassphraseEnc = "";
    String note = "";
    /** 最近一次连通的往返毫秒；-1 = 未知 / 不通（展示用） */
    long lastRttMs = -1;
    long lastSeen = 0;

    JumpServer() {
        this.id = UUID.randomUUID().toString();
    }

    // ------------------------------------------------------------------
    // 展示与推导
    // ------------------------------------------------------------------

    static int defaultPortFor(String protocol) {
        return PROTO_TELNET.equals(protocol) ? DEFAULT_TELNET_PORT : DEFAULT_SSH_PORT;
    }

    /** 端口：没写就用协议默认 */
    int port() {
        int index = host.lastIndexOf(':');
        if (index > 0 && index < host.length() - 1) {
            try {
                return Integer.parseInt(host.substring(index + 1));
            } catch (NumberFormatException ignored) {
                // 落回协议默认
            }
        }
        return defaultPortFor(protocol);
    }

    /** 去掉端口后的主机名 / IP */
    String hostname() {
        int index = host.lastIndexOf(':');
        String value = index > 0 ? host.substring(0, index) : host;
        return value.trim();
    }

    boolean isSsh() {
        return !PROTO_TELNET.equals(protocol);
    }

    /** 列表行的第二行：user@host:port */
    String addressLabel() {
        String user = JumpStore.text(username);
        return (user.isEmpty() ? "" : user + "@") + host;
    }

    String protocolLabel() {
        return PROTO_TELNET.equals(protocol) ? "Telnet" : "SSH";
    }

    String authLabel() {
        return AUTH_KEY.equals(authType) ? "密钥" : "密码";
    }

    /**
     * 解析 host 输入：容忍 user@host:port / host / 整条 URL。
     * user 部分摘出来放进 username（输入框没单独填时）。
     */
    void applyHostInput(String raw) {
        String value = JumpStore.text(raw);
        if (value.isEmpty()) {
            return;
        }
        int scheme = value.indexOf("://");
        if (scheme >= 0) {
            value = value.substring(scheme + 3);
        }
        int slash = value.indexOf('/');
        if (slash >= 0) {
            value = value.substring(0, slash);
        }
        int at = value.lastIndexOf('@');
        if (at > 0) {
            String user = value.substring(0, at).trim();
            if (JumpStore.text(username).isEmpty()) {
                username = user;
            }
            value = value.substring(at + 1);
        }
        // 只写 host 没写端口时补协议默认端口（显式归一，展示与连接共用一个口径）
        if (!value.contains(":")) {
            value = value + ":" + defaultPortFor(protocol);
        }
        host = value;
    }

    JSONObject toJson() throws JSONException {
        JSONObject object = new JSONObject();
        object.put("id", id);
        object.put("name", JumpStore.text(name));
        object.put("protocol", JumpStore.text(protocol));
        object.put("host", JumpStore.text(host));
        object.put("username", JumpStore.text(username));
        object.put("authType", JumpStore.text(authType));
        object.put("passwordEnc", JumpStore.text(passwordEnc));
        object.put("keyId", JumpStore.text(keyId));
        object.put("keyPassphraseEnc", JumpStore.text(keyPassphraseEnc));
        object.put("note", JumpStore.text(note));
        object.put("lastRttMs", lastRttMs);
        object.put("lastSeen", lastSeen);
        return object;
    }

    static JumpServer fromJson(JSONObject object) {
        JumpServer server = new JumpServer();
        String id = object.optString("id", "");
        if (!id.isEmpty()) {
            server.id = id;
        }
        server.name = object.optString("name", "");
        String protocol = object.optString("protocol", PROTO_SSH);
        server.protocol = PROTO_TELNET.equals(protocol) ? PROTO_TELNET : PROTO_SSH;
        server.host = object.optString("host", "");
        server.username = object.optString("username", "");
        String auth = object.optString("authType", AUTH_PASSWORD);
        server.authType = AUTH_KEY.equals(auth) ? AUTH_KEY : AUTH_PASSWORD;
        server.passwordEnc = object.optString("passwordEnc", "");
        server.keyId = object.optString("keyId", "");
        server.keyPassphraseEnc = object.optString("keyPassphraseEnc", "");
        server.note = object.optString("note", "");
        server.lastRttMs = object.optLong("lastRttMs", -1);
        server.lastSeen = object.optLong("lastSeen", 0);
        return server;
    }

    JumpServer copy() {
        JumpServer other = new JumpServer();
        other.id = id;
        other.name = name;
        other.protocol = protocol;
        other.host = host;
        other.username = username;
        other.authType = authType;
        other.passwordEnc = passwordEnc;
        other.keyId = keyId;
        other.keyPassphraseEnc = keyPassphraseEnc;
        other.note = note;
        other.lastRttMs = lastRttMs;
        other.lastSeen = lastSeen;
        return other;
    }
}
