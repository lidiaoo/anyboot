package com.anyboot.remote;

import android.content.Context;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;

/**
 * 服务器（跳板机）与密钥的持久化 + 终端偏好 + 会话历史。
 *
 * <p>全部存 SharedPreferences（与 ServerStore 同一个 prefs 文件，备份策略一致）。
 * 敏感字段（密码 / 私钥 / 口令）先经 {@link KeyVault}（Android Keystore AES-GCM）
 * 加密再落盘 —— 配置文件里只有密文。
 */
final class JumpStore {

    private static final String KEY_JUMPS = "jump_servers";
    private static final String KEY_KEYS = "ssh_keys";
    private static final String KEY_HISTORY = "session_history";
    private static final String KEY_TERMINAL_DARK = "terminal_dark";
    private static final String KEY_TERMINAL_FONT = "terminal_font_size";
    private static final String KEY_TERMINAL_FONT_FAMILY = "terminal_font_family";
    private static final String KEY_TERMINAL_AUTO_KEYBOARD = "terminal_auto_keyboard";
    private static final String KEY_TERMINAL_KEY_BAR = "terminal_key_bar";
    private static final String KEY_GITHUB_CACHE = "github_cache";
    private static final String KEY_TRUSTED_HOSTS = "trusted_hosts";

    private JumpStore() {
    }

    static String text(String raw) {
        return raw == null ? "" : raw.trim();
    }

    // ------------------------------------------------------------------
    // 跳板机
    // ------------------------------------------------------------------

    static List<JumpServer> loadJumps(Context context) {
        List<JumpServer> list = new ArrayList<>();
        String raw = ServerStore.prefs(context).getString(KEY_JUMPS, "");
        if (raw != null && !raw.trim().isEmpty()) {
            try {
                JSONArray array = new JSONArray(raw);
                for (int i = 0; i < array.length(); i++) {
                    JSONObject object = array.optJSONObject(i);
                    if (object != null) {
                        list.add(JumpServer.fromJson(object));
                    }
                }
            } catch (Exception ignored) {
                // 配置损坏就当没有，别让 App 起不来
            }
        }
        return list;
    }

    static void saveJumps(Context context, List<JumpServer> list) {
        JSONArray array = new JSONArray();
        for (JumpServer server : list) {
            try {
                array.put(server.toJson());
            } catch (Exception ignored) {
                // 单条写不进去就跳过
            }
        }
        ServerStore.prefs(context).edit().putString(KEY_JUMPS, array.toString()).apply();
    }

    static JumpServer findJump(List<JumpServer> list, String id) {
        if (id == null || id.isEmpty()) {
            return null;
        }
        for (JumpServer server : list) {
            if (id.equals(server.id)) {
                return server;
            }
        }
        return null;
    }

    /**
     * 应急通道（连接前手输密码）里勾了「保存密码」：加密后写回那条跳板机，下次连接免输。
     *
     * @return true = 已落盘；false = 密码为空 / 加密失败（Keystore 异常）/ 配置里找不到该条
     */
    static boolean rememberPassword(Context context, String jumpId, String password) {
        if (text(password).isEmpty()) {
            return false;
        }
        String encrypted = KeyVault.encrypt(password);
        if (encrypted.isEmpty()) {
            // 宁可这次不落盘，也不能把明文写进配置
            return false;
        }
        List<JumpServer> list = loadJumps(context);
        JumpServer target = findJump(list, jumpId);
        if (target == null) {
            return false;
        }
        target.passwordEnc = encrypted;
        saveJumps(context, list);
        return true;
    }

    /** 有几台电脑经由这台跳板机（详情页「经由此服务器的电脑」） */
    static int computersVia(Context context, String jumpId) {
        int count = 0;
        for (ServerProfile profile : ServerStore.load(context)) {
            if (jumpId.equals(profile.jumpServerId)) {
                count++;
            }
        }
        return count;
    }

    // ------------------------------------------------------------------
    // 密钥
    // ------------------------------------------------------------------

    static List<SshKeyEntry> loadKeys(Context context) {
        List<SshKeyEntry> list = new ArrayList<>();
        String raw = ServerStore.prefs(context).getString(KEY_KEYS, "");
        if (raw != null && !raw.trim().isEmpty()) {
            try {
                JSONArray array = new JSONArray(raw);
                for (int i = 0; i < array.length(); i++) {
                    JSONObject object = array.optJSONObject(i);
                    if (object != null) {
                        list.add(SshKeyEntry.fromJson(object));
                    }
                }
            } catch (Exception ignored) {
                // 同上：损坏就当没有
            }
        }
        return list;
    }

    static void saveKeys(Context context, List<SshKeyEntry> list) {
        JSONArray array = new JSONArray();
        for (SshKeyEntry entry : list) {
            try {
                array.put(entry.toJson());
            } catch (Exception ignored) {
                // 单条失败跳过
            }
        }
        ServerStore.prefs(context).edit().putString(KEY_KEYS, array.toString()).apply();
    }

    static SshKeyEntry findKey(List<SshKeyEntry> list, String id) {
        if (id == null || id.isEmpty()) {
            return null;
        }
        for (SshKeyEntry entry : list) {
            if (id.equals(entry.id)) {
                return entry;
            }
        }
        return null;
    }

    /** 有几台服务器在用这把密钥（删除前给用户看关联数） */
    static int keysInUse(List<JumpServer> jumps, String keyId) {
        int count = 0;
        for (JumpServer server : jumps) {
            if (keyId.equals(server.keyId)) {
                count++;
            }
        }
        return count;
    }

    // ------------------------------------------------------------------
    // 会话历史（终端页「历史」）
    // ------------------------------------------------------------------

    /** 一条终端会话历史 */
    static final class HistoryEntry {
        String serverId;
        String serverName;
        long startedAt;
        long endedAt;
    }

    static List<HistoryEntry> loadHistory(Context context) {
        List<HistoryEntry> list = new ArrayList<>();
        String raw = ServerStore.prefs(context).getString(KEY_HISTORY, "");
        if (raw != null && !raw.trim().isEmpty()) {
            try {
                JSONArray array = new JSONArray(raw);
                for (int i = 0; i < array.length(); i++) {
                    JSONObject object = array.optJSONObject(i);
                    if (object == null) {
                        continue;
                    }
                    HistoryEntry entry = new HistoryEntry();
                    entry.serverId = object.optString("serverId", "");
                    entry.serverName = object.optString("serverName", "");
                    entry.startedAt = object.optLong("startedAt", 0);
                    entry.endedAt = object.optLong("endedAt", 0);
                    list.add(entry);
                }
            } catch (Exception ignored) {
                // 同上
            }
        }
        return list;
    }

    static void appendHistory(Context context, HistoryEntry entry) {
        List<HistoryEntry> list = loadHistory(context);
        list.add(0, entry);
        // 只留最近 50 条，防 prefs 无限膨胀
        while (list.size() > 50) {
            list.remove(list.size() - 1);
        }
        JSONArray array = new JSONArray();
        for (HistoryEntry item : list) {
            try {
                JSONObject object = new JSONObject();
                object.put("serverId", item.serverId);
                object.put("serverName", item.serverName);
                object.put("startedAt", item.startedAt);
                object.put("endedAt", item.endedAt);
                array.put(object);
            } catch (Exception ignored) {
                // 单条失败跳过
            }
        }
        ServerStore.prefs(context).edit().putString(KEY_HISTORY, array.toString()).apply();
    }

    // ------------------------------------------------------------------
    // 终端偏好（「终端设置」页 + 终端页顶栏的 🌓 / A- / A+ 共用同一份）
    // ------------------------------------------------------------------

    /** 终端字号范围与默认值（设置页与终端页都从这里取，避免两处各写一份） */
    static final int TERMINAL_FONT_DEFAULT = 14;
    static final int TERMINAL_FONT_MIN = 8;
    static final int TERMINAL_FONT_MAX = 28;

    static boolean terminalDark(Context context) {
        return ServerStore.prefs(context).getBoolean(KEY_TERMINAL_DARK, true);
    }

    static void setTerminalDark(Context context, boolean dark) {
        ServerStore.prefs(context).edit().putBoolean(KEY_TERMINAL_DARK, dark).apply();
    }

    static int terminalFontSize(Context context) {
        int value = ServerStore.prefs(context).getInt(KEY_TERMINAL_FONT, TERMINAL_FONT_DEFAULT);
        if (value < TERMINAL_FONT_MIN) {
            return TERMINAL_FONT_MIN;
        }
        return value > TERMINAL_FONT_MAX ? TERMINAL_FONT_MAX : value;
    }

    static void setTerminalFontSize(Context context, int size) {
        int value = size < TERMINAL_FONT_MIN ? TERMINAL_FONT_MIN
                : (size > TERMINAL_FONT_MAX ? TERMINAL_FONT_MAX : size);
        ServerStore.prefs(context).edit().putInt(KEY_TERMINAL_FONT, value).apply();
    }

    /** 终端字体族（xterm 的 CSS font-family）：空 = 内置 Menlo/等宽栈。
     *  字体缺失时 WebView 自动回落，不会白屏 —— 不做存在性检查。 */
    static String terminalFontFamily(Context context) {
        String value = ServerStore.prefs(context).getString(KEY_TERMINAL_FONT_FAMILY, "");
        return value == null ? "" : value.trim();
    }

    static void setTerminalFontFamily(Context context, String family) {
        String value = family == null ? "" : family.trim();
        ServerStore.prefs(context).edit().putString(KEY_TERMINAL_FONT_FAMILY, value).apply();
    }

    /** 进终端就自动把软键盘弹出来（默认关：老行为是先看到输出，要打字时自己点） */
    static boolean terminalAutoKeyboard(Context context) {
        return ServerStore.prefs(context).getBoolean(KEY_TERMINAL_AUTO_KEYBOARD, false);
    }

    static void setTerminalAutoKeyboard(Context context, boolean auto) {
        ServerStore.prefs(context).edit().putBoolean(KEY_TERMINAL_AUTO_KEYBOARD, auto).apply();
    }

    /** 终端底部的快捷键栏（Esc/Tab/Ctrl/方向键/Enter），默认开 */
    static boolean terminalKeyBar(Context context) {
        return ServerStore.prefs(context).getBoolean(KEY_TERMINAL_KEY_BAR, true);
    }

    static void setTerminalKeyBar(Context context, boolean show) {
        ServerStore.prefs(context).edit().putBoolean(KEY_TERMINAL_KEY_BAR, show).apply();
    }

    // ------------------------------------------------------------------
    // GitHub 缓存 / TOFU 指纹
    // ------------------------------------------------------------------

    static String githubCache(Context context) {
        return ServerStore.prefs(context).getString(KEY_GITHUB_CACHE, "");
    }

    static void setGithubCache(Context context, String json) {
        ServerStore.prefs(context).edit().putString(KEY_GITHUB_CACHE, json == null ? "" : json).apply();
    }

    /** TOFU 已信任的主机指纹：host:port → "keytype base64" */
    static String trustedHostKey(Context context, String hostPort) {
        return ServerStore.prefs(context).getString(KEY_TRUSTED_HOSTS + "_" + hash(hostPort), "");
    }

    static void setTrustedHostKey(Context context, String hostPort, String value) {
        ServerStore.prefs(context).edit()
                .putString(KEY_TRUSTED_HOSTS + "_" + hash(hostPort), value == null ? "" : value)
                .apply();
    }

    private static String hash(String raw) {
        return String.valueOf((raw == null ? "" : raw).hashCode());
    }
}
