package com.anyboot.remote;

import android.content.Context;
import android.content.SharedPreferences;
import android.util.Base64;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.text.SimpleDateFormat;
import java.util.ArrayList;
import java.util.Date;
import java.util.HashSet;
import java.util.Iterator;
import java.util.List;
import java.util.Locale;
import java.util.Set;
import java.util.TimeZone;
import java.util.UUID;

import javax.crypto.Cipher;
import javax.crypto.SecretKeyFactory;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.PBEKeySpec;
import javax.crypto.spec.SecretKeySpec;

/**
 * 备份核心（docs/backup-design.md §3/§4/§5）。
 *
 * <p>信封契约与 iOS / 桌面逐字节对齐：
 * <ul>
 *   <li>顶层：format="anyboot-backup" / version=1 / encryption / exportedAt / includes / data</li>
 *   <li>canonical 键 camelCase（Android 原生存档里的 snake_case 配对键在导出时改名）</li>
 *   <li>secrets 明文表：key:&lt;id&gt;:private / jump:&lt;id&gt;:password …（Android Keystore 密文
 *       跨机不可解，导出必须经 {@link KeyVault#decrypt} 解成明文）</li>
 *   <li>加密：PBKDF2-HMAC-SHA256 310000 轮 + AES-256-GCM，AAD 与 iOS/桌面同串</li>
 * </ul>
 *
 * <p>导入端丢弃备份里的 ref/enc 引用，统一经本端 {@link KeyVault} 重新加密落盘。
 */
final class BackupManager {

    static final String FORMAT = "anyboot-backup";
    static final int VERSION = 1;
    static final int MAX_FILE_BYTES = 10 * 1024 * 1024;
    static final int PBKDF2_ROUNDS = 310_000;
    static final String SCHEME = "PBKDF2-SHA256-310000 + AES-256-GCM";

    /** 分类 id（includes / data 键 / 墓碑 cat 都用它） */
    static final String[] CATEGORIES = {"servers", "jumps", "keys", "terminal", "history"};

    static String labelOf(String cat) {
        switch (cat) {
            case "servers": return "电脑列表";
            case "jumps": return "SSH 服务器";
            case "keys": return "SSH 密钥";
            case "terminal": return "偏好设置";
            case "history": return "连接记录";
            default: return cat;
        }
    }

    private BackupManager() {
    }

    // ==================================================================
    // 收集（导出组包）
    // ==================================================================

    /** 组明文信封；categories 为勾选分类；sync 为 null 表示手工导出 */
    static JSONObject collect(Context context, Set<String> categories, JSONObject sync) throws Exception {
        JSONObject data = new JSONObject();

        if (categories.contains("servers")) {
            JSONArray items = new JSONArray();
            for (ServerProfile profile : ServerStore.load(context)) {
                items.put(canonicalServer(profile));
            }
            JSONObject servers = new JSONObject();
            servers.put("items", items);
            servers.put("activeId", ServerStore.activeId(context));
            data.put("servers", servers);
        }
        if (categories.contains("jumps")) {
            JSONArray items = new JSONArray();
            for (JumpServer jump : JumpStore.loadJumps(context)) {
                items.put(canonicalJump(jump));
            }
            JSONObject jumps = new JSONObject();
            jumps.put("items", items);
            data.put("jumps", jumps);
        }
        if (categories.contains("keys")) {
            JSONArray items = new JSONArray();
            JSONObject secrets = new JSONObject();
            for (SshKeyEntry entry : JumpStore.loadKeys(context)) {
                items.put(canonicalKey(entry));
                // Keystore 密文跨机是废数据：解成明文进 secrets（iOS 同口径）
                String pem = KeyVault.decrypt(entry.privateEnc);
                if (!pem.isEmpty()) {
                    secrets.put("key:" + entry.id + ":private", pem);
                }
            }
            JSONObject keys = new JSONObject();
            keys.put("items", items);
            keys.put("secrets", secrets);
            data.put("keys", keys);
        }
        if (categories.contains("terminal")) {
            JSONObject terminal = new JSONObject();
            terminal.put("terminalDark", JumpStore.terminalDark(context));
            terminal.put("terminalFontSize", JumpStore.terminalFontSize(context));
            terminal.put("terminalFontFamily", JumpStore.terminalFontFamily(context));
            terminal.put("terminalAutoKeyboard", JumpStore.terminalAutoKeyboard(context));
            terminal.put("terminalKeyBar", JumpStore.terminalKeyBar(context));
            terminal.put("transport", ServerStore.transport(context));
            terminal.put("connectAll", ServerStore.connectAll(context));
            data.put("terminal", terminal);
        }
        if (categories.contains("history")) {
            JSONArray sessions = new JSONArray();
            for (JumpStore.HistoryEntry entry : JumpStore.loadHistory(context)) {
                JSONObject item = new JSONObject();
                item.put("serverId", entry.serverId);
                item.put("serverName", entry.serverName);
                item.put("startedAt", isoMs(entry.startedAt));
                item.put("endedAt", isoMs(entry.endedAt));
                sessions.put(item);
            }
            // Android 的 TOFU 键是 hash(hostPort)，不可逆 → 只导会话；导入方向支持 TOFU
            JSONObject history = new JSONObject();
            history.put("sessions", sessions);
            history.put("trustedHosts", new JSONArray());
            data.put("history", history);
        }

        JSONObject envelope = new JSONObject();
        envelope.put("format", FORMAT);
        envelope.put("version", VERSION);
        envelope.put("app", appVersion());
        envelope.put("platform", "android");
        envelope.put("exportedAt", isoNow());
        JSONArray includes = new JSONArray();
        for (String cat : CATEGORIES) {
            if (categories.contains(cat)) {
                includes.put(cat);
            }
        }
        envelope.put("includes", includes);
        if (sync != null) {
            envelope.put("sync", sync);
        }
        envelope.put("data", data);
        return envelope;
    }

    /** 序列化 + 可选加密 → 可写文件字节 */
    static byte[] serialize(JSONObject envelope, String password) throws Exception {
        byte[] plain = envelope.toString().getBytes(StandardCharsets.UTF_8);
        if (password == null || password.isEmpty()) {
            return plain;
        }
        byte[] salt = new byte[16];
        byte[] iv = new byte[12];
        java.security.SecureRandom random = new java.security.SecureRandom();
        random.nextBytes(salt);
        random.nextBytes(iv);
        String saltB64 = Base64.encodeToString(salt, Base64.NO_WRAP);
        String ivB64 = Base64.encodeToString(iv, Base64.NO_WRAP);

        JSONObject encryption = new JSONObject();
        encryption.put("scheme", SCHEME);
        encryption.put("salt", saltB64);
        encryption.put("iv", ivB64);
        encryption.put("rounds", PBKDF2_ROUNDS);

        byte[] ciphertext = aesGcm(plain, password, salt, iv, aadSeed(saltB64, ivB64, PBKDF2_ROUNDS), true);

        // 外层：data 置空、payload 装密文
        JSONObject boxed = new JSONObject(envelope.toString());
        boxed.remove("data");
        boxed.put("encryption", encryption);
        boxed.put("payload", Base64.encodeToString(ciphertext, Base64.NO_WRAP));
        return boxed.toString().getBytes(StandardCharsets.UTF_8);
    }

    // ==================================================================
    // 解析（导入 / 同步拉取侧）
    // ==================================================================

    /** 原始字节 → 信封。加密文件走 {@link #decrypt}。 */
    static JSONObject parse(byte[] raw) throws Exception {
        if (raw.length > MAX_FILE_BYTES) {
            throw new BackupException("文件超过 10MB，已拒绝");
        }
        JSONObject envelope = new JSONObject(new String(raw, StandardCharsets.UTF_8));
        requireFormat(envelope);
        checkVersion(envelope);
        return envelope;
    }

    static boolean isEncrypted(byte[] raw) throws Exception {
        JSONObject envelope = new JSONObject(new String(raw, StandardCharsets.UTF_8));
        return envelope.has("payload") && envelope.has("encryption");
    }

    static JSONObject decrypt(byte[] raw, String password) throws Exception {
        if (raw.length > MAX_FILE_BYTES) {
            throw new BackupException("文件超过 10MB，已拒绝");
        }
        JSONObject envelope = new JSONObject(new String(raw, StandardCharsets.UTF_8));
        requireFormat(envelope);
        if (!envelope.has("payload") || !envelope.has("encryption")) {
            checkVersion(envelope);
            return envelope;   // 未加密文件带口令调用：直接走明文路径
        }
        JSONObject encryption = envelope.getJSONObject("encryption");
        String saltB64 = encryption.optString("salt", "");
        String ivB64 = encryption.optString("iv", "");
        int rounds = encryption.optInt("rounds", PBKDF2_ROUNDS);
        byte[] combined;
        byte[] salt;
        byte[] iv;
        try {
            combined = Base64.decode(envelope.getString("payload"), Base64.NO_WRAP);
            salt = Base64.decode(saltB64, Base64.NO_WRAP);
            iv = Base64.decode(ivB64, Base64.NO_WRAP);
        } catch (IllegalArgumentException error) {
            throw new BackupException("备份文件已损坏");
        }
        if (iv.length != 12) {
            throw new BackupException("备份文件已损坏");
        }
        byte[] plain;
        try {
            plain = aesGcm(combined, password, salt, iv, aadSeed(saltB64, ivB64, rounds), false);
        } catch (Exception error) {
            throw new BackupException("口令错误或文件已损坏");
        }
        JSONObject inner = new JSONObject(new String(plain, StandardCharsets.UTF_8));
        requireFormat(inner);
        checkVersion(inner);
        if (envelope.has("sync")) {
            inner.put("sync", envelope.getJSONObject("sync"));
        }
        return inner;
    }

    private static void requireFormat(JSONObject envelope) throws Exception {
        if (!FORMAT.equals(envelope.optString("format", ""))) {
            throw new BackupException("不是有效的 AnyBoot 备份文件");
        }
    }

    private static void checkVersion(JSONObject envelope) throws Exception {
        if (envelope.optInt("version", 1) > VERSION) {
            throw new BackupException("备份来自更新版本，请先升级 App");
        }
    }

    // ==================================================================
    // 摘要（S3 预览）
    // ==================================================================

    /** rows: cat → {backup, local, conflicts[]}；missingJumpRefs */
    static JSONObject summarize(Context context, JSONObject envelope) throws Exception {
        JSONObject summary = new JSONObject();
        JSONObject rows = new JSONObject();
        JSONObject data = envelope.optJSONObject("data");
        if (data == null) {
            summary.put("rows", rows);
            summary.put("missingJumpRefs", 0);
            return summary;
        }

        JSONObject servers = data.optJSONObject("servers");
        if (servers != null) {
            List<String> localIds = new ArrayList<>();
            List<ServerProfile> local = ServerStore.load(context);
            for (ServerProfile profile : local) {
                localIds.add(profile.id);
            }
            rows.put("servers", rowOf(context, servers, localIds, "电脑"));
        }
        JSONObject jumps = data.optJSONObject("jumps");
        if (jumps != null) {
            List<JumpServer> local = JumpStore.loadJumps(context);
            List<String> localIds = new ArrayList<>();
            for (JumpServer jump : local) {
                localIds.add(jump.id);
            }
            rows.put("jumps", rowOf(context, jumps, localIds, "服务器"));
        }
        JSONObject keys = data.optJSONObject("keys");
        if (keys != null) {
            List<SshKeyEntry> local = JumpStore.loadKeys(context);
            List<String> localIds = new ArrayList<>();
            for (SshKeyEntry entry : local) {
                localIds.add(entry.id);
            }
            rows.put("keys", rowOf(context, keys, localIds, "密钥"));
        }
        if (data.has("terminal")) {
            JSONObject row = new JSONObject();
            row.put("backup", 1);
            row.put("local", 1);
            row.put("conflicts", new JSONArray());
            rows.put("terminal", row);
        }
        JSONObject history = data.optJSONObject("history");
        if (history != null) {
            JSONObject row = new JSONObject();
            row.put("backup", history.optJSONArray("sessions") == null ? 0
                    : history.optJSONArray("sessions").length());
            row.put("local", JumpStore.loadHistory(context).size());
            row.put("conflicts", new JSONArray());
            rows.put("history", row);
        }

        // 引用完整性
        int missing = 0;
        Set<String> jumpIds = new HashSet<>();
        JSONObject jumpData = data.optJSONObject("jumps");
        if (jumpData != null) {
            JSONArray items = jumpData.optJSONArray("items");
            if (items != null) {
                for (int i = 0; i < items.length(); i++) {
                    jumpIds.add(items.optJSONObject(i).optString("id", ""));
                }
            }
        }
        JSONObject serverData = data.optJSONObject("servers");
        if (serverData != null) {
            JSONArray items = serverData.optJSONArray("items");
            if (items != null) {
                for (int i = 0; i < items.length(); i++) {
                    String ref = items.optJSONObject(i).optString("jumpServerId", "");
                    if (!ref.isEmpty() && !jumpIds.contains(ref)) {
                        missing++;
                    }
                }
            }
        }
        summary.put("rows", rows);
        summary.put("missingJumpRefs", missing);
        summary.put("platform", envelope.optString("platform", ""));
        summary.put("exportedAt", envelope.optString("exportedAt", ""));
        summary.put("app", envelope.optString("app", ""));
        return summary;
    }

    private static JSONObject rowOf(Context context, JSONObject payload, List<String> localIds,
                                    String kindName) throws Exception {
        JSONArray items = payload.optJSONArray("items");
        int backup = items == null ? 0 : items.length();
        JSONArray conflicts = new JSONArray();
        if (items != null) {
            for (int i = 0; i < items.length(); i++) {
                String id = items.optJSONObject(i).optString("id", "");
                if (localIds.contains(id)) {
                    conflicts.put(kindName + " " + (i + 1));
                }
            }
        }
        JSONObject row = new JSONObject();
        row.put("backup", backup);
        row.put("local", localIds.size());
        row.put("conflicts", conflicts);
        return row;
    }

    // ==================================================================
    // 落地（导入 apply / 同步合并共用）
    // ==================================================================

    /** categories 勾选分类；strategy: "skipExisting" / "overwrite"；结果报告见 {@link Report} */
    static Report apply(Context context, JSONObject envelope, Set<String> categories,
                        String strategy, Set<String> tombstones) throws Exception {
        Report report = new Report();
        JSONObject data = envelope.optJSONObject("data");
        if (data == null) {
            return report;
        }
        JSONObject secrets = data.optJSONObject("keys") == null
                ? new JSONObject() : data.optJSONObject("keys").optJSONObject("secrets");
        if (secrets == null) {
            secrets = new JSONObject();
        }

        if (categories.contains("servers") && data.has("servers")) {
            report.servers = applyServers(context, data.getJSONObject("servers"), strategy, tombstones);
        }
        if (categories.contains("jumps") && data.has("jumps")) {
            report.jumps = applyJumps(context, data.getJSONObject("jumps"), strategy, tombstones, secrets);
        }
        if (categories.contains("keys") && data.has("keys")) {
            report.keys = applyKeys(context, data.getJSONObject("keys"), strategy, tombstones, secrets);
        }
        if (categories.contains("terminal") && data.has("terminal")) {
            report.terminal = applyPrefs(context, data.getJSONObject("terminal"));
        }
        if (categories.contains("history") && data.has("history")) {
            report.history = applyHistory(context, data.getJSONObject("history"));
        }
        return report;
    }

    private static JSONObject applyServers(Context context, JSONObject payload, String strategy,
                                           Set<String> tombstones) throws Exception {
        JSONObject row = newRow("servers");
        JSONArray items = payload.optJSONArray("items");
        int added = 0, updated = 0, skipped = 0;
        JSONArray failed = new JSONArray();
        List<ServerProfile> local = ServerStore.load(context);
        ServerProfile active = null;

        if ("overwrite".equals(strategy)) {
            List<ServerProfile> replacement = new ArrayList<>();
            if (items != null) {
                for (int i = 0; i < items.length(); i++) {
                    JSONObject item = items.optJSONObject(i);
                    if (item == null) continue;
                    try {
                        ServerProfile profile = serverFromCanonical(item);
                        profile.updatedAt = System.currentTimeMillis();
                        replacement.add(profile);
                    } catch (Exception error) {
                        failed.put("第 " + (i + 1) + " 台电脑：档案字段不合法");
                    }
                }
            }
            String activeId = payload.optString("activeId", "");
            ServerStore.save(context, replacement);
            ServerStore.setActiveId(context, replacementContains(replacement, activeId) ? activeId
                    : (replacement.isEmpty() ? "" : replacement.get(0).id));
            row.put("added", replacement.size());
            row.put("failed", failed);
            return row;
        }

        boolean dirty = false;
        if (items != null) {
            for (int i = 0; i < items.length(); i++) {
                JSONObject item = items.optJSONObject(i);
                if (item == null) continue;
                ServerProfile profile;
                try {
                    profile = serverFromCanonical(item);
                } catch (Exception error) {
                    failed.put("第 " + (i + 1) + " 台电脑：档案字段不合法");
                    continue;
                }
                String id = profile.id;
                if (tombstones.contains("servers|" + id)) {
                    removeFromList(local, id);
                    dirty = true;
                    skipped++;
                    continue;
                }
                ServerProfile existing = ServerStore.find(local, id);
                if (existing != null && "skipExisting".equals(strategy)) {
                    skipped++;
                    continue;
                }
                profile.updatedAt = System.currentTimeMillis();
                if (existing != null) {
                    local.set(local.indexOf(existing), profile);
                    updated++;
                } else {
                    local.add(profile);
                    added++;
                }
                dirty = true;
            }
        }
        if (dirty) {
            ServerStore.sort(local);
            ServerStore.save(context, local);
        }
        row.put("added", added);
        row.put("updated", updated);
        row.put("skipped", skipped);
        row.put("failed", failed);
        return row;
    }

    private static boolean replacementContains(List<ServerProfile> list, String id) {
        for (ServerProfile profile : list) {
            if (profile.id.equals(id)) return true;
        }
        return false;
    }

    private static void removeFromList(List<ServerProfile> list, String id) {
        for (int i = list.size() - 1; i >= 0; i--) {
            if (list.get(i).id.equals(id)) {
                list.remove(i);
            }
        }
    }

    private static JSONObject applyJumps(Context context, JSONObject payload, String strategy,
                                         Set<String> tombstones, JSONObject secrets) throws Exception {
        JSONObject row = newRow("jumps");
        JSONArray items = payload.optJSONArray("items");
        List<JumpServer> local = JumpStore.loadJumps(context);
        int added = 0, updated = 0, skipped = 0;
        JSONArray failed = new JSONArray();

        if ("overwrite".equals(strategy)) {
            for (JumpServer old : local) {
                if (!old.passwordEnc.isEmpty()) old.passwordEnc = "";
                if (!old.keyPassphraseEnc.isEmpty()) old.keyPassphraseEnc = "";
            }
            local.clear();
        }
        boolean dirty = "overwrite".equals(strategy);
        if (items != null) {
            for (int i = 0; i < items.length(); i++) {
                JSONObject item = items.optJSONObject(i);
                if (item == null) continue;
                JumpServer jump;
                try {
                    jump = jumpFromCanonical(item, secrets);
                } catch (Exception error) {
                    failed.put("第 " + (i + 1) + " 台服务器：档案字段不合法");
                    continue;
                }
                String id = jump.id;
                if (tombstones.contains("jumps|" + id)) {
                    removeFromJumps(local, id);
                    dirty = true;
                    skipped++;
                    continue;
                }
                JumpServer existing = JumpStore.findJump(local, id);
                if (existing != null && "skipExisting".equals(strategy)) {
                    skipped++;
                    continue;
                }
                jump.updatedAt = System.currentTimeMillis();
                if (existing != null) {
                    local.set(local.indexOf(existing), jump);
                    updated++;
                } else {
                    local.add(jump);
                    added++;
                }
                dirty = true;
            }
        }
        if (dirty) {
            JumpStore.saveJumps(context, local);
        }
        row.put("added", added);
        row.put("updated", updated);
        row.put("skipped", skipped);
        row.put("failed", failed);
        return row;
    }

    private static void removeFromJumps(List<JumpServer> list, String id) {
        for (int i = list.size() - 1; i >= 0; i--) {
            if (list.get(i).id.equals(id)) {
                list.remove(i);
            }
        }
    }

    private static JSONObject applyKeys(Context context, JSONObject payload, String strategy,
                                        Set<String> tombstones, JSONObject secrets) throws Exception {
        JSONObject row = newRow("keys");
        JSONArray items = payload.optJSONArray("items");
        List<SshKeyEntry> local = JumpStore.loadKeys(context);
        int added = 0, updated = 0, skipped = 0;
        JSONArray failed = new JSONArray();

        if ("overwrite".equals(strategy)) {
            Set<String> keepIds = new HashSet<>();
            if (items != null) {
                for (int i = 0; i < items.length(); i++) {
                    keepIds.add(items.optJSONObject(i).optString("id", ""));
                }
            }
            for (int i = local.size() - 1; i >= 0; i--) {
                if (!keepIds.contains(local.get(i).id)) {
                    local.remove(i);
                }
            }
        }
        boolean dirty = "overwrite".equals(strategy);
        if (items != null) {
            for (int i = 0; i < items.length(); i++) {
                JSONObject item = items.optJSONObject(i);
                if (item == null) continue;
                String id = item.optString("id", "").trim();
                if (id.isEmpty()) {
                    failed.put("第 " + (i + 1) + " 把密钥：元数据字段不合法");
                    continue;
                }
                if (tombstones.contains("keys|" + id)) {
                    removeFromKeys(local, id);
                    dirty = true;
                    skipped++;
                    continue;
                }
                SshKeyEntry existing = JumpStore.findKey(local, id);
                if (existing != null && "skipExisting".equals(strategy)) {
                    skipped++;
                    continue;
                }
                SshKeyEntry entry = keyFromCanonical(item);
                // 私钥：重新经本端 Keystore 加密落盘（备份里的 ref/enc 一律丢弃）
                entry.privateEnc = "";
                String pem = secrets.optString("key:" + id + ":private", "");
                if (!pem.isEmpty()) {
                    String encrypted = KeyVault.encrypt(pem);
                    if (encrypted.isEmpty()) {
                        failed.put("密钥「" + entry.name + "」：私钥加密失败（设备密钥库异常），已跳过私钥");
                    } else {
                        entry.privateEnc = encrypted;
                    }
                } else if (existing == null) {
                    failed.put("密钥「" + entry.name + "」：备份中无私钥材料（明文备份？）");
                }
                entry.updatedAt = System.currentTimeMillis();
                if (existing != null) {
                    local.set(local.indexOf(existing), entry);
                    updated++;
                } else {
                    local.add(entry);
                    added++;
                }
                dirty = true;
            }
        }
        if (dirty) {
            JumpStore.saveKeys(context, local);
        }
        row.put("added", added);
        row.put("updated", updated);
        row.put("skipped", skipped);
        row.put("failed", failed);
        return row;
    }

    private static void removeFromKeys(List<SshKeyEntry> list, String id) {
        for (int i = list.size() - 1; i >= 0; i--) {
            if (list.get(i).id.equals(id)) {
                list.remove(i);
            }
        }
    }

    static JSONObject applyPrefs(Context context, JSONObject prefs) throws Exception {
        JSONObject row = newRow("terminal");
        if (prefs.has("terminalDark")) {
            JumpStore.setTerminalDark(context, prefs.optBoolean("terminalDark", true));
        }
        if (prefs.has("terminalFontSize")) {
            JumpStore.setTerminalFontSize(context, prefs.optInt("terminalFontSize", 14));
        }
        if (prefs.has("terminalFontFamily")) {
            JumpStore.setTerminalFontFamily(context, prefs.optString("terminalFontFamily", ""));
        }
        if (prefs.has("terminalAutoKeyboard")) {
            JumpStore.setTerminalAutoKeyboard(context, prefs.optBoolean("terminalAutoKeyboard", false));
        }
        if (prefs.has("terminalKeyBar")) {
            JumpStore.setTerminalKeyBar(context, prefs.optBoolean("terminalKeyBar", true));
        }
        if (prefs.has("transport")) {
            String transport = prefs.optString("transport", "auto");
            // 值域过滤（§2.3）：auto/ws/http 之外（含桌面异常值）回落 auto
            if (!"auto".equals(transport) && !"ws".equals(transport) && !"http".equals(transport)) {
                transport = "auto";
            }
            ServerStore.setTransport(context, transport);
        }
        if (prefs.has("connectAll")) {
            ServerStore.setConnectAll(context, prefs.optBoolean("connectAll", false));
        }
        row.put("added", 1);
        return row;
    }

    static JSONObject applyHistory(Context context, JSONObject history) throws Exception {
        JSONObject row = newRow("history");
        List<JumpStore.HistoryEntry> local = JumpStore.loadHistory(context);
        Set<String> seen = new HashSet<>();
        for (JumpStore.HistoryEntry entry : local) {
            seen.add(entry.serverId + "|" + entry.startedAt);
        }
        int added = 0, skipped = 0;
        JSONArray failed = new JSONArray();
        JSONArray sessions = history.optJSONArray("sessions");
        if (sessions != null) {
            List<JumpStore.HistoryEntry> incoming = new ArrayList<>();
            for (int i = 0; i < sessions.length(); i++) {
                JSONObject item = sessions.optJSONObject(i);
                if (item == null) continue;
                long start = msIso(item.optString("startedAt", ""));
                long end = msIso(item.optString("endedAt", ""));
                if (start <= 0 || end <= 0) {
                    failed.put("会话「" + item.optString("serverName", "") + "」：时间字段不合法");
                    continue;
                }
                String key = item.optString("serverId", "") + "|" + start;
                if (seen.contains(key)) {
                    skipped++;
                    continue;
                }
                seen.add(key);
                JumpStore.HistoryEntry entry = new JumpStore.HistoryEntry();
                entry.serverId = item.optString("serverId", "");
                entry.serverName = item.optString("serverName", "");
                entry.startedAt = start;
                entry.endedAt = end;
                incoming.add(entry);
                added++;
            }
            // 追加到本地并保持最新在前、50 条上限
            local.addAll(incoming);
            java.util.Collections.sort(local, (a, b) -> Long.compare(b.startedAt, a.startedAt));
            while (local.size() > 50) {
                local.remove(local.size() - 1);
            }
            JSONArray array = new JSONArray();
            for (JumpStore.HistoryEntry entry : local) {
                try {
                    JSONObject object = new JSONObject();
                    object.put("serverId", entry.serverId);
                    object.put("serverName", entry.serverName);
                    object.put("startedAt", entry.startedAt);
                    object.put("endedAt", entry.endedAt);
                    array.put(object);
                } catch (Exception ignored) {
                    // 单条失败跳过
                }
            }
            ServerStore.prefs(context).edit()
                    .putString("session_history", array.toString()).apply();
        }
        // TOFU 指纹：host:port → 本端 hash 键
        JSONArray hosts = history.optJSONArray("trustedHosts");
        if (hosts != null) {
            SharedPreferences.Editor editor = ServerStore.prefs(context).edit();
            int count = 0;
            for (int i = 0; i < hosts.length(); i++) {
                JSONObject item = hosts.optJSONObject(i);
                if (item == null) continue;
                String host = item.optString("host", "");
                String fingerprint = item.optString("fingerprint", "");
                if (host.isEmpty()) continue;
                editor.putString("trusted_hosts_" + JumpStore.hash(host), fingerprint);
                count++;
            }
            editor.apply();
            //Android 的 TOFU 键是 hashCode(hostPort) —— 与 iOS/桌面的动态键口径一致
            if (count > 0) {
                row.put("added", count);
            }
        }
        row.put("added", row.optInt("added", 0) + added);
        row.put("skipped", skipped);
        row.put("failed", failed);
        return row;
    }

    private static JSONObject newRow(String cat) throws Exception {
        JSONObject row = new JSONObject();
        row.put("category", cat);
        row.put("added", 0);
        row.put("updated", 0);
        row.put("skipped", 0);
        row.put("failed", new JSONArray());
        return row;
    }

    // ==================================================================
    // canonical ⇄ 本端模型
    // ==================================================================

    /** Android 原生档案 → canonical（配对键 snake_case 改 camelCase；时间转 ISO） */
    static JSONObject canonicalServer(ServerProfile profile) throws Exception {
        JSONObject item = new JSONObject(profile.toJson().toString());
        renamePairKeysToCanonical(item);
        if (profile.updatedAt > 0) {
            item.put("updatedAt", isoMs(profile.updatedAt));
        }
        return item;
    }

    static JSONObject canonicalJump(JumpServer jump) throws Exception {
        JSONObject item = new JSONObject();
        item.put("id", jump.id);
        item.put("name", JumpStore.text(jump.name));
        item.put("protocol", JumpStore.text(jump.protocol));
        item.put("host", JumpStore.text(jump.host));
        item.put("username", JumpStore.text(jump.username));
        item.put("authType", JumpStore.text(jump.authType));
        item.put("keyId", JumpStore.text(jump.keyId));
        item.put("jumpServerId", JumpStore.text(jump.jumpServerId));
        item.put("note", JumpStore.text(jump.note));
        item.put("termPath", JumpStore.text(jump.termPath));
        item.put("termShell", JumpStore.text(jump.termShell));
        item.put("lastRttMs", jump.lastRttMs);
        if (jump.lastSeen > 0) {
            item.put("lastSeen", isoMs(jump.lastSeen));
        }
        if (jump.updatedAt > 0) {
            item.put("updatedAt", isoMs(jump.updatedAt));
        }
        return item;
    }

    static JSONObject canonicalKey(SshKeyEntry entry) throws Exception {
        JSONObject item = new JSONObject();
        item.put("id", entry.id);
        item.put("name", JumpStore.text(entry.name));
        item.put("algorithm", JumpStore.text(entry.algorithm));
        item.put("fingerprint", JumpStore.text(entry.fingerprint));
        item.put("publicKey", JumpStore.text(entry.publicKey));
        item.put("createdAt", isoMs(entry.createdAt));
        if (entry.updatedAt > 0) {
            item.put("updatedAt", isoMs(entry.updatedAt));
        }
        return item;
    }

    /** canonical 配对键 → Android 原生键（其余键名 fromJson 本来就兼容） */
    private static void renamePairKeysToNative(JSONObject item) {
        try {
            JSONArray pairs = item.optJSONArray("pairs");
            if (pairs != null) {
                for (int i = 0; i < pairs.length(); i++) {
                    JSONObject pair = pairs.optJSONObject(i);
                    if (pair == null) continue;
                    renameKey(pair, "probeTimeoutMs", "probe_timeout_ms");
                    renameKey(pair, "tunnelId", "tunnel_id");
                    renameKey(pair, "createdAt", "created_at");
                    // ISO createdAt → 毫秒（桌面/移动端 ISO 进来时）
                    String created = pair.optString("created_at", "");
                    if (!created.isEmpty() && !created.matches("\\d+")) {
                        long ms = msIso(created);
                        if (ms > 0) {
                            pair.put("created_at", ms);
                        }
                    }
                }
            }
            renameKey(item, "blockProbeMode", "block_probe_mode");
            renameKey(item, "blockTimeoutsMs", "block_timeouts_ms");
        } catch (Exception ignored) {
            // 单字段改名失败按原样走：fromJson 对未知键本就宽容
        }
    }

    private static void renamePairKeysToCanonical(JSONObject item) {
        try {
            JSONArray pairs = item.optJSONArray("pairs");
            if (pairs != null) {
                for (int i = 0; i < pairs.length(); i++) {
                    JSONObject pair = pairs.optJSONObject(i);
                    if (pair == null) continue;
                    renameKey(pair, "probe_timeout_ms", "probeTimeoutMs");
                    renameKey(pair, "tunnel_id", "tunnelId");
                    renameKey(pair, "created_at", "createdAt");
                }
            }
            renameKey(item, "block_probe_mode", "blockProbeMode");
            renameKey(item, "block_timeouts_ms", "blockTimeoutsMs");
        } catch (Exception ignored) {
            // 同上
        }
    }

    private static void renameKey(JSONObject object, String from, String to) {
        if (object.has(from) && !from.equals(to)) {
            try {
                object.put(to, object.get(from));
                object.remove(from);
            } catch (Exception ignored) {
            }
        }
    }

    private static ServerProfile serverFromCanonical(JSONObject item) throws Exception {
        JSONObject nativeJson = new JSONObject(item.toString());
        renamePairKeysToNative(nativeJson);
        // 桌面端 structured addresses 在移动端由 host 承载：host 空时取第一个地址
        if (JumpStore.text(nativeJson.optString("host", "")).isEmpty()) {
            JSONArray addresses = nativeJson.optJSONArray("addresses");
            if (addresses != null && addresses.length() > 0) {
                nativeJson.put("host", addresses.optString(0, ""));
            }
        }
        ServerProfile profile = ServerProfile.fromJson(nativeJson);
        if (JumpStore.text(profile.id).isEmpty() || profile.addresses.isEmpty()) {
            throw new BackupException("档案字段不合法");
        }
        long ms = msIso(item.optString("updatedAt", ""));
        if (ms > 0) {
            profile.updatedAt = ms;
        }
        return profile;
    }

    private static JumpServer jumpFromCanonical(JSONObject item, JSONObject secrets) throws Exception {
        JumpServer jump = new JumpServer();
        String id = JumpStore.text(item.optString("id", ""));
        if (id.isEmpty()) {
            throw new BackupException("档案字段不合法");
        }
        jump.id = id;
        jump.name = item.optString("name", "");
        jump.protocol = "telnet".equals(item.optString("protocol", "ssh"))
                ? JumpServer.PROTO_TELNET : JumpServer.PROTO_SSH;
        String host = JumpStore.text(item.optString("host", ""));
        if (host.isEmpty()) {
            JSONArray addresses = item.optJSONArray("addresses");
            if (addresses != null && addresses.length() > 0) {
                host = JumpStore.text(addresses.optString(0, ""));
            }
        }
        if (host.isEmpty()) {
            throw new BackupException("档案字段不合法");
        }
        jump.host = host;
        jump.username = item.optString("username", "");
        jump.authType = "key".equals(item.optString("authType", "password"))
                ? JumpServer.AUTH_KEY : JumpServer.AUTH_PASSWORD;
        jump.keyId = item.optString("keyId", "");
        jump.jumpServerId = item.optString("jumpServerId", "");
        jump.note = item.optString("note", "");
        jump.termPath = item.optString("termPath", "");
        jump.termShell = item.optString("termShell", "");
        jump.lastRttMs = (long) item.optDouble("lastRttMs", -1);
        long lastSeen = msIso(item.optString("lastSeen", ""));
        jump.lastSeen = lastSeen > 0 ? lastSeen : item.optLong("lastSeen", 0);
        // secrets：明文 → 本端 Keystore 加密落盘
        String password = secrets.optString("jump:" + id + ":password", "");
        if (!password.isEmpty()) {
            jump.passwordEnc = KeyVault.encrypt(password);
        }
        String passphrase = secrets.optString("jump:" + id + ":keyPassphrase", "");
        if (!passphrase.isEmpty()) {
            jump.keyPassphraseEnc = KeyVault.encrypt(passphrase);
        }
        long updated = msIso(item.optString("updatedAt", ""));
        jump.updatedAt = updated;
        return jump;
    }

    private static SshKeyEntry keyFromCanonical(JSONObject item) throws Exception {
        SshKeyEntry entry = new SshKeyEntry();
        entry.id = JumpStore.text(item.optString("id", ""));
        entry.name = item.optString("name", "");
        entry.algorithm = item.optString("algorithm", "");
        entry.fingerprint = item.optString("fingerprint", "");
        entry.publicKey = item.optString("publicKey", "");
        long created = msIso(item.optString("createdAt", ""));
        entry.createdAt = created > 0 ? created : System.currentTimeMillis();
        return entry;
    }

    // ==================================================================
    // AES-256-GCM（与 iOS BackupCrypto / 桌面 backup_crypto.rs 同参）
    // ==================================================================

    static String aadSeed(String saltB64, String ivB64, int rounds) {
        return "anyboot-backup|v1|" + SCHEME + "|" + saltB64 + "|" + ivB64 + "|" + rounds;
    }

    static byte[] aesGcm(byte[] input, String password, byte[] salt, byte[] iv,
                         String aad, boolean encrypt) throws Exception {
        PBEKeySpec spec = new PBEKeySpec(password.toCharArray(), salt, PBKDF2_ROUNDS, 256);
        SecretKeyFactory factory = SecretKeyFactory.getInstance("PBKDF2WithHmacSHA256");
        byte[] key = factory.generateSecret(spec).getEncoded();
        Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
        cipher.init(encrypt ? Cipher.ENCRYPT_MODE : Cipher.DECRYPT_MODE,
                new SecretKeySpec(key, "AES"), new GCMParameterSpec(128, iv));
        cipher.updateAAD(aad.getBytes(StandardCharsets.UTF_8));
        return cipher.doFinal(input);
    }

    // ==================================================================
    // 时间与版本
    // ==================================================================

    static String isoNow() {
        return isoMs(System.currentTimeMillis());
    }

    /** 毫秒 → ISO8601（UTC，与 iOS/桌面同口径） */
    static String isoMs(long ms) {
        if (ms <= 0) return "";
        SimpleDateFormat format = new SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss'Z'", Locale.US);
        format.setTimeZone(TimeZone.getTimeZone("UTC"));
        return format.format(new Date(ms));
    }

    /** ISO8601 → 毫秒；解析失败给 0 */
    static long msIso(String iso) {
        String value = JumpStore.text(iso);
        if (value.isEmpty()) return 0;
        if (value.matches("\\d+")) {
            try {
                return Long.parseLong(value);
            } catch (NumberFormatException ignored) {
                return 0;
            }
        }
        String[] patterns = {
                "yyyy-MM-dd'T'HH:mm:ss'Z'",
                "yyyy-MM-dd'T'HH:mm:ss.SSS'Z'",
                "yyyy-MM-dd'T'HH:mm:ssXXX",
        };
        for (String pattern : patterns) {
            try {
                SimpleDateFormat format = new SimpleDateFormat(pattern, Locale.US);
                format.setTimeZone(TimeZone.getTimeZone("UTC"));
                Date date = format.parse(value);
                if (date != null) return date.getTime();
            } catch (Exception ignored) {
                // 尝试下一个格式
            }
        }
        return 0;
    }

    private static String appVersion() {
        return "1.2";
    }

    // ==================================================================
    // 报告
    // ==================================================================

    /** 导入/合并报告（UI 结果页消费） */
    static final class Report {
        JSONObject servers;
        JSONObject jumps;
        JSONObject keys;
        JSONObject terminal;
        JSONObject history;

        int totalAdded() {
            return sum("added");
        }

        int totalUpdated() {
            return sum("updated");
        }

        int totalSkipped() {
            return sum("skipped");
        }

        int totalFailed() {
            int count = 0;
            for (JSONObject row : new JSONObject[]{servers, jumps, keys, terminal, history}) {
                if (row != null) {
                    count += row.optJSONArray("failed") == null ? 0
                            : row.optJSONArray("failed").length();
                }
            }
            return count;
        }

        private int sum(String key) {
            int total = 0;
            for (JSONObject row : new JSONObject[]{servers, jumps, keys, terminal, history}) {
                if (row != null) total += row.optInt(key, 0);
            }
            return total;
        }
    }

    /** 带 UI 文案的备份异常（§6 错误码→文案表） */
    static final class BackupException extends Exception {
        BackupException(String message) {
            super(message);
        }
    }
}
