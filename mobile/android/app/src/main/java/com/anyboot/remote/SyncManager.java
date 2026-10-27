package com.anyboot.remote;

import android.content.Context;
import android.content.SharedPreferences;
import android.os.Handler;
import android.os.Looper;
import android.util.Base64;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.UUID;

/**
 * WebDAV 云同步（docs/backup-design.md §9）。
 *
 * <p>整文件拉取 → 按条目 LWW 合并（updatedAt 大者胜、nil 视为极旧）→ 推回；
 * 云端单文件 anyboot-sync.json，rev + If-Match 乐观锁，冲突再拉一轮合并后重推一次。
 * 互斥：同一时刻只允许一轮同步（手动与去抖共用）。
 */
final class SyncManager {

    private static final Object LOCK = new Object();
    private static volatile boolean running = false;
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static final long DEBOUNCE_MS = 30_000L;
    private static Runnable pendingDebounce;

    private SyncManager() {
    }

    // ==================================================================
    // 配置（S2 表单持久化；密码/口令进 KeyVault）
    // ==================================================================

    static final class Config {
        String serverUrl = "";
        String username = "";
        String passwordRef = "webdav-password";
        String directory = "AnyBoot";
        int periodMinutes = 0;      // 0 = 仅手动
        boolean syncOnChange = false;
        Set<String> categories = new HashSet<>();
        boolean encrypt = false;
        String encryptPasswordRef = "sync-encrypt-password";

        static Config load(Context context) {
            Config config = new Config();
            try {
                JSONObject object = new JSONObject(
                        ServerStore.prefs(context).getString("backup_sync_config", "{}"));
                config.serverUrl = object.optString("serverUrl", "");
                config.username = object.optString("username", "");
                config.passwordRef = object.optString("passwordRef", "webdav-password");
                config.directory = object.optString("directory", "AnyBoot");
                config.periodMinutes = object.optInt("periodMinutes", 0);
                config.syncOnChange = object.optBoolean("syncOnChange", false);
                JSONArray cats = object.optJSONArray("categories");
                if (cats != null) {
                    for (int i = 0; i < cats.length(); i++) {
                        config.categories.add(cats.optString(i, ""));
                    }
                }
                config.encrypt = object.optBoolean("encrypt", false);
                config.encryptPasswordRef = object.optString("encryptPasswordRef", "sync-encrypt-password");
            } catch (Exception ignored) {
                // 配置损坏 = 未配置
            }
            return config;
        }

        void save(Context context) {
            try {
                JSONObject object = new JSONObject();
                object.put("serverUrl", serverUrl);
                object.put("username", username);
                object.put("passwordRef", passwordRef);
                object.put("directory", directory);
                object.put("periodMinutes", periodMinutes);
                object.put("syncOnChange", syncOnChange);
                JSONArray cats = new JSONArray();
                for (String cat : categories) {
                    cats.put(cat);
                }
                object.put("categories", cats);
                object.put("encrypt", encrypt);
                object.put("encryptPasswordRef", encryptPasswordRef);
                ServerStore.prefs(context).edit()
                        .putString("backup_sync_config", object.toString()).apply();
            } catch (Exception ignored) {
            }
        }

        boolean isConfigured() {
            return serverUrl.trim().startsWith("http://") || serverUrl.trim().startsWith("https://");
        }

        String password(Context context) {
            return KeyVault.decrypt(ServerStore.prefs(context).getString(passwordRef, ""));
        }

        String encryptPassword(Context context) {
            return KeyVault.decrypt(ServerStore.prefs(context).getString(encryptPasswordRef, ""));
        }

        String savePassword(Context context, String plain) {
            String encrypted = KeyVault.encrypt(plain);
            if (!encrypted.isEmpty()) {
                ServerStore.prefs(context).edit().putString(passwordRef, encrypted).apply();
            }
            return passwordRef;
        }

        String saveEncryptPassword(Context context, String plain) {
            String encrypted = KeyVault.encrypt(plain);
            if (!encrypted.isEmpty()) {
                ServerStore.prefs(context).edit().putString(encryptPasswordRef, encrypted).apply();
            }
            return encryptPasswordRef;
        }

        /** 文件资源 URL：serverUrl + directory + anyboot-sync.json */
        String fileUrl() {
            String base = serverUrl.trim();
            if (base.endsWith("/")) base = base.substring(0, base.length() - 1);
            String dir = cleanDirectory();
            return base + (dir.isEmpty() ? "" : "/" + dir) + "/anyboot-sync.json";
        }

        String cleanDirectory() {
            String value = directory.trim();
            while (value.startsWith("/")) value = value.substring(1);
            while (value.endsWith("/")) value = value.substring(0, value.length() - 1);
            return value;
        }
    }

    // ==================================================================
    // WebDAV 三动词（HttpURLConnection；必须在子线程调用）
    // ==================================================================

    private static byte[] readStream(InputStream stream) {
        if (stream == null) return new byte[0];
        try (InputStream in = stream) {
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            byte[] chunk = new byte[8192];
            int count;
            while ((count = in.read(chunk)) > 0) {
                out.write(chunk, 0, count);
                if (out.size() > BackupManager.MAX_FILE_BYTES) {
                    throw new Exception("文件超过 10MB，已拒绝");
                }
            }
            return out.toByteArray();
        } catch (Exception ignored) {
            return new byte[0];
        }
    }

    /** 状态码 → 用户文案（§6 错误码表） */
    static String messageFor(int code, byte[] body) {
        switch (code) {
            case 200: case 207: case 201: case 204:
                return null;
            case 401:
                return "401 用户名或密码错误";
            case 404:
                return "404 远端目录不存在";
            case 412:
                return "云端已被其他设备更新，请重试";
            case 507:
                return "WebDAV 空间不足";
            case -1:
                String reason = new String(body, StandardCharsets.UTF_8);
                return "连接失败：" + (reason.isEmpty() ? "网络超时" : reason);
            default:
                return "服务器无响应（" + code + "），请稍后重试";
        }
    }

    static final class DavResult {
        final int code;
        final byte[] body;
        final String etag;

        DavResult(int code, byte[] body, String etag) {
            this.code = code;
            this.body = body;
            this.etag = etag;
        }
    }

    // ==================================================================
    // 引擎入口（都在子线程跑，结果回主线程）
    // ==================================================================

    /** 回调在主线程 */
    public interface SyncCallback {
        void onDone(String errorOrNull);
    }

    /** 立即同步；errorOrNull = null 成功 */
    public static void syncNow(Context context, SyncCallback callback) {
        synchronized (LOCK) {
            if (running) {
                post(callback, "正在同步，请稍后");
                return;
            }
            running = true;
        }
        new Thread(() -> {
            String error = runOnce(context.getApplicationContext());
            synchronized (LOCK) {
                running = false;
            }
            post(callback, error);
        }, "anyboot-sync").start();
    }

    /** 记录变化触发的去抖同步 */
    public static void notifyChanged(Context context) {
        Config config = Config.load(context);
        if (!config.isConfigured() || !config.syncOnChange) return;
        if (!ServerStore.prefs(context).getBoolean("backup_sync_enabled", false)) return;
        synchronized (LOCK) {
            if (running) return;
            if (pendingDebounce != null) {
                MAIN.removeCallbacks(pendingDebounce);
            }
            Runnable task = () -> syncNow(context, null);
            pendingDebounce = task;
            MAIN.postDelayed(task, DEBOUNCE_MS);
        }
    }

    /** 测试连接（子线程发起、主线程回调）。code 404 时返回 notFound 标记 */
    public static void testConnection(Context context, Config config, TestCallback callback) {
        new Thread(() -> {
            String url = config.fileUrl();
            String dirUrl = url.substring(0, url.lastIndexOf('/'));
            String credentials = config.username + ":" + config.password(context);
            DavResult result = requestWithAuth(dirUrl, "PROPFIND", null, null, credentials);
            int code = result.code;
            if (code == 200 || code == 207) {
                post(callback, "ok", null);
            } else if (code == 404) {
                post(callback, "notFound", null);
            } else {
                post(callback, "failed", messageFor(code, result.body));
            }
        }, "anyboot-dav-test").start();
    }

    public interface TestCallback {
        void onDone(String status, String errorOrNull);
    }

    /** 创建远端目录（逐级 MKCOL） */
    public static void createDirectory(Context context, Config config, SyncCallback callback) {
        new Thread(() -> {
            String credentials = config.username + ":" + config.password(context);
            String base = config.serverUrl.trim();
            if (base.endsWith("/")) base = base.substring(0, base.length() - 1);
            String error = null;
            for (String part : config.cleanDirectory().split("/")) {
                if (part.isEmpty()) continue;
                base = base + "/" + part;
                DavResult result = requestWithAuth(base, "MKCOL", new byte[0], null, credentials);
                int code = result.code;
                if (!(code == 201 || code == 200 || code == 204 || code == 405)) {
                    error = messageFor(code, result.body);
                    break;
                }
            }
            String finalError = error;
            post(callback, finalError);
        }, "anyboot-dav-mkcol").start();
    }

    private static DavResult requestWithAuth(String url, String method, byte[] body,
                                             String ifMatch, String credentials) {
        HttpURLConnection connection = null;
        try {
            connection = (HttpURLConnection) new URL(url).openConnection();
            connection.setConnectTimeout(20_000);
            connection.setReadTimeout(30_000);
            connection.setRequestMethod(method);
            connection.setRequestProperty("Authorization", "Basic "
                    + Base64.encodeToString(credentials.getBytes(StandardCharsets.UTF_8), Base64.NO_WRAP));
            if ("PROPFIND".equals(method)) {
                connection.setRequestProperty("Depth", "0");
            }
            if (ifMatch != null && !ifMatch.isEmpty()) {
                connection.setRequestProperty("If-Match", "\"" + ifMatch + "\"");
            }
            if (body != null && body.length > 0) {
                connection.setDoOutput(true);
                connection.setRequestProperty("Content-Type", "application/json");
                try (OutputStream out = connection.getOutputStream()) {
                    out.write(body);
                }
            }
            int code = connection.getResponseCode();
            byte[] response = readStream(code >= 400 ? connection.getErrorStream()
                    : connection.getInputStream());
            String etag = connection.getHeaderField("ETag");
            if (etag != null) {
                etag = etag.replace("\"", "");
            }
            return new DavResult(code, response, etag);
        } catch (Exception error) {
            return new DavResult(-1, error.getMessage() == null
                    ? new byte[0] : error.getMessage().getBytes(StandardCharsets.UTF_8), null);
        } finally {
            if (connection != null) {
                connection.disconnect();
            }
        }
    }

    // ==================================================================
    // 单轮拉推（子线程）
    // ==================================================================

    private static String runOnce(Context context) {
        Config config = Config.load(context);
        if (!config.isConfigured()) {
            return "请先配置 WebDAV 服务器和同步内容";
        }
        String credentials = config.username + ":" + config.password(context);
        String fileUrl = config.fileUrl();
        SharedPreferences prefs = ServerStore.prefs(context);
        int rev = prefs.getInt("backup_sync_rev", 0);
        String etag = prefs.getString("backup_sync_etag", "");

        // ---- pull ----
        DavResult got = requestWithAuth(fileUrl, "GET", null, null, credentials);
        String davError = messageFor(got.code, got.body);
        if (got.code != 200 && got.code != 404) {
            return davError != null ? davError : "拉取失败";
        }

        int remoteRev = rev;
        if (got.code == 200) {
            JSONObject envelope;
            try {
                envelope = openEnvelope(context, config, got.body);
            } catch (Exception error) {
                return error.getMessage();
            }
            try {
                JSONObject sync = envelope.optJSONObject("sync");
                if (sync != null) {
                    remoteRev = sync.optInt("rev", rev);
                }
                boolean changed = mergeIntoLocal(context, envelope, config);
                if (!changed) {
                    prefs.edit().putInt("backup_sync_rev", remoteRev).apply();
                    return null;   // 双端一致，不必推
                }
            } catch (Exception error) {
                return "合并失败：" + error.getMessage();
            }
        }

        // ---- push ----
        String password = config.encrypt ? config.encryptPassword(context) : null;
        if (config.encrypt && (password == null || password.isEmpty())) {
            return "已开启同步加密但未设置口令";
        }
        try {
            return push(context, config, credentials, fileUrl, remoteRev + 1,
                    etag, password, prefs);
        } catch (Exception error) {
            return "上传失败：" + error.getMessage();
        }
    }

    private static String push(Context context, Config config, String credentials, String fileUrl,
                               int newRev, String ifMatch, String password,
                               SharedPreferences prefs) throws Exception {
        JSONObject sync = new JSONObject();
        sync.put("deviceId", deviceId(context));
        sync.put("rev", newRev);
        JSONArray tombstones = new JSONArray(
                prefs.getString("backup_sync_tombstones", "[]"));
        sync.put("tombstones", tombstones);
        JSONObject envelope = BackupManager.collect(context, config.categories, sync);
        byte[] payload = BackupManager.serialize(envelope, password);

        DavResult put = requestWithAuth(fileUrl, "PUT", payload, ifMatch, credentials);
        if (put.code == 200 || put.code == 201 || put.code == 204) {
            prefs.edit().putInt("backup_sync_rev", newRev)
                    .putString("backup_sync_etag", put.etag == null ? "" : put.etag).apply();
            pruneTombstones(prefs);
            return null;
        }
        if (put.code == 412) {
            // 云端被其他设备更新过：再拉一轮合并后重推一次（防活锁只做一轮）
            DavResult got = requestWithAuth(fileUrl, "GET", null, null, credentials);
            remoteRevAgain(context, config, got, prefs);
            JSONObject retrySync = new JSONObject();
            retrySync.put("deviceId", deviceId(context));
            retrySync.put("rev", prefs.getInt("backup_sync_rev", 0) + 1);
            retrySync.put("tombstones", new JSONArray(
                    prefs.getString("backup_sync_tombstones", "[]")));
            JSONObject retryEnvelope = BackupManager.collect(context, config.categories, retrySync);
            byte[] retryPayload = BackupManager.serialize(retryEnvelope, password);
            DavResult retryPut = requestWithAuth(fileUrl, "PUT", retryPayload, null, credentials);
            if (retryPut.code == 200 || retryPut.code == 201 || retryPut.code == 204) {
                prefs.edit().putInt("backup_sync_rev", retrySync.optInt("rev"))
                        .putString("backup_sync_etag", retryPut.etag == null ? "" : retryPut.etag).apply();
                pruneTombstones(prefs);
                return null;
            }
            return messageFor(retryPut.code, retryPut.body);
        }
        return messageFor(put.code, put.body);
    }

    private static void remoteRevAgain(Context context, Config config, DavResult got,
                                       SharedPreferences prefs) throws Exception {
        if (got.code != 200) return;
        JSONObject envelope = openEnvelope(context, config, got.body);
        mergeIntoLocal(context, envelope, config);
        JSONObject sync = envelope.optJSONObject("sync");
        if (sync != null) {
            prefs.edit().putInt("backup_sync_rev", sync.optInt("rev")).apply();
        }
    }

    private static JSONObject openEnvelope(Context context, Config config, byte[] raw) throws Exception {
        if (BackupManager.isEncrypted(raw)) {
            String password = config.encryptPassword(context);
            if (password.isEmpty()) {
                throw new BackupManager.BackupException("云端备份已加密，请先在本机配置同步口令");
            }
            return BackupManager.decrypt(raw, password);
        }
        return BackupManager.parse(raw);
    }

    private static String deviceId(Context context) {
        SharedPreferences prefs = ServerStore.prefs(context);
        String id = prefs.getString("backup_device_id", "");
        if (id.isEmpty()) {
            id = UUID.randomUUID().toString();
            prefs.edit().putString("backup_device_id", id).apply();
        }
        return id;
    }

    // ==================================================================
    // LWW 合并（与 iOS SyncEngine 同语义表）
    // ==================================================================

    /** @return 本机是否有改动（决定要不要推） */
    private static boolean mergeIntoLocal(Context context, JSONObject envelope,
                                          Config config) throws Exception {
        Set<String> tombstones = readTombstones(envelope);
        JSONObject data = envelope.optJSONObject("data");
        boolean changed = false;

        if (data == null) return false;
        JSONObject secrets = data.optJSONObject("keys") == null
                ? new JSONObject() : data.optJSONObject("keys").optJSONObject("secrets");
        if (secrets == null) secrets = new JSONObject();

        if (config.categories.contains("servers")) {
            JSONObject payload = data.optJSONObject("servers");
            if (payload != null) {
                changed |= mergeServers(context, payload, tombstones);
            }
        }
        if (config.categories.contains("jumps")) {
            JSONObject payload = data.optJSONObject("jumps");
            if (payload != null) {
                changed |= mergeJumps(context, payload, tombstones, secrets);
            }
        }
        if (config.categories.contains("keys")) {
            JSONObject payload = data.optJSONObject("keys");
            if (payload != null) {
                changed |= mergeKeys(context, payload, tombstones, secrets);
            }
        }
        if (config.categories.contains("terminal") && data.has("terminal")) {
            BackupManager.applyPrefs(context, data.getJSONObject("terminal"));
            changed = true;
        }
        if (config.categories.contains("history")) {
            JSONObject payload = data.optJSONObject("history");
            if (payload != null) {
                JSONObject report = BackupManager.applyHistory(context, payload);
                if (report.optInt("added", 0) > 0) changed = true;
            }
        }
        return changed;
    }

    private static Set<String> readTombstones(JSONObject envelope) {
        Set<String> set = new HashSet<>();
        try {
            JSONObject sync = envelope.optJSONObject("sync");
            JSONArray list = sync == null ? null : sync.optJSONArray("tombstones");
            if (list != null) {
                for (int i = 0; i < list.length(); i++) {
                    JSONObject item = list.optJSONObject(i);
                    if (item != null) {
                        set.add(item.optString("cat", "") + "|" + item.optString("id", ""));
                    }
                }
            }
        } catch (Exception ignored) {
        }
        return set;
    }

    private static boolean mergeServers(Context context, JSONObject payload,
                                        Set<String> tombstones) throws Exception {
        boolean changed = false;
        List<ServerProfile> local = ServerStore.load(context);
        JSONArray items = payload.optJSONArray("items");
        Set<String> remoteIds = new HashSet<>();
        if (items != null) {
            for (int i = 0; i < items.length(); i++) {
                JSONObject item = items.optJSONObject(i);
                if (item == null) continue;
                String id = item.optString("id", "");
                if (id.isEmpty()) continue;
                remoteIds.add(id);
                if (tombstones.contains("servers|" + id)) {
                    int before = local.size();
                    removeFromList(local, id);
                    if (local.size() != before) changed = true;
                    continue;
                }
                ServerProfile incoming;
                try {
                    incoming = serverFrom(item);
                } catch (Exception ignored) {
                    continue;
                }
                ServerProfile existing = ServerStore.find(local, id);
                if (existing == null || newer(incoming.updatedAt, existing.updatedAt)) {
                    if (existing != null) {
                        local.set(local.indexOf(existing), incoming);
                    } else {
                        local.add(incoming);
                    }
                    changed = true;
                }
            }
        }
        // 本机有、远端没有 → 记墓碑（下次推给其他设备）
        for (ServerProfile profile : local) {
            if (!remoteIds.contains(profile.id) && !tombstones.contains("servers|" + profile.id)) {
                addTombstone(context, "servers", profile.id);
            }
        }
        if (changed) {
            ServerStore.sort(local);
            ServerStore.save(context, local);
        }
        return changed;
    }

    private static ServerProfile serverFrom(JSONObject item) throws Exception {
        JSONObject nativeJson = new JSONObject(item.toString());
        // 配对键改回原生（BackupManager 私有方法复用：走公开 collect 反向路径）
        JSONArray pairs = nativeJson.optJSONArray("pairs");
        if (pairs != null) {
            for (int i = 0; i < pairs.length(); i++) {
                JSONObject pair = pairs.optJSONObject(i);
                if (pair == null) continue;
                rename(pair, "probeTimeoutMs", "probe_timeout_ms");
                rename(pair, "tunnelId", "tunnel_id");
                rename(pair, "createdAt", "created_at");
                String created = pair.optString("created_at", "");
                if (!created.isEmpty() && !created.matches("\\d+")) {
                    long ms = BackupManager.msIso(created);
                    if (ms > 0) pair.put("created_at", ms);
                }
            }
        }
        rename(nativeJson, "blockProbeMode", "block_probe_mode");
        rename(nativeJson, "blockTimeoutsMs", "block_timeouts_ms");
        if (JumpStore.text(nativeJson.optString("host", "")).isEmpty()) {
            JSONArray addresses = nativeJson.optJSONArray("addresses");
            if (addresses != null && addresses.length() > 0) {
                nativeJson.put("host", addresses.optString(0, ""));
            }
        }
        ServerProfile profile = ServerProfile.fromJson(nativeJson);
        if (JumpStore.text(profile.id).isEmpty() || profile.addresses.isEmpty()) {
            throw new Exception("bad server");
        }
        long ms = BackupManager.msIso(item.optString("updatedAt", ""));
        if (ms > 0) {
            profile.updatedAt = ms;
        } else {
            profile.updatedAt = System.currentTimeMillis();
        }
        return profile;
    }

    private static void rename(JSONObject object, String from, String to) {
        try {
            if (object.has(from)) {
                object.put(to, object.get(from));
                object.remove(from);
            }
        } catch (Exception ignored) {
        }
    }

    private static boolean mergeJumps(Context context, JSONObject payload, Set<String> tombstones,
                                      JSONObject secrets) throws Exception {
        boolean changed = false;
        List<JumpServer> local = JumpStore.loadJumps(context);
        JSONArray items = payload.optJSONArray("items");
        Set<String> remoteIds = new HashSet<>();
        if (items != null) {
            for (int i = 0; i < items.length(); i++) {
                JSONObject item = items.optJSONObject(i);
                if (item == null) continue;
                String id = item.optString("id", "");
                if (id.isEmpty()) continue;
                remoteIds.add(id);
                if (tombstones.contains("jumps|" + id)) {
                    int before = local.size();
                    removeFromJumps(local, id);
                    if (local.size() != before) changed = true;
                    continue;
                }
                JumpServer incoming = jumpFrom(item, secrets);
                JumpServer existing = JumpStore.findJump(local, id);
                if (existing == null || newer(incoming.updatedAt, existing.updatedAt)) {
                    if (existing != null) {
                        local.set(local.indexOf(existing), incoming);
                    } else {
                        local.add(incoming);
                    }
                    changed = true;
                }
            }
        }
        for (JumpServer jump : local) {
            if (!remoteIds.contains(jump.id) && !tombstones.contains("jumps|" + jump.id)) {
                addTombstone(context, "jumps", jump.id);
            }
        }
        if (changed) {
            JumpStore.saveJumps(context, local);
        }
        return changed;
    }

    private static JumpServer jumpFrom(JSONObject item, JSONObject secrets) throws Exception {
        JumpServer jump = new JumpServer();
        String id = JumpStore.text(item.optString("id", ""));
        if (id.isEmpty()) throw new Exception("bad jump");
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
        if (host.isEmpty()) throw new Exception("bad jump");
        jump.host = host;
        jump.username = item.optString("username", "");
        jump.authType = "key".equals(item.optString("authType", "password"))
                ? JumpServer.AUTH_KEY : JumpServer.AUTH_PASSWORD;
        jump.keyId = item.optString("keyId", "");
        jump.jumpServerId = item.optString("jumpServerId", "");
        jump.note = item.optString("note", "");
        jump.termPath = item.optString("termPath", "");
        jump.termShell = item.optString("termShell", "");
        String password = secrets.optString("jump:" + id + ":password", "");
        if (!password.isEmpty()) {
            jump.passwordEnc = KeyVault.encrypt(password);
        }
        String passphrase = secrets.optString("jump:" + id + ":keyPassphrase", "");
        if (!passphrase.isEmpty()) {
            jump.keyPassphraseEnc = KeyVault.encrypt(passphrase);
        }
        long ms = BackupManager.msIso(item.optString("updatedAt", ""));
        jump.updatedAt = ms > 0 ? ms : System.currentTimeMillis();
        return jump;
    }

    private static boolean mergeKeys(Context context, JSONObject payload, Set<String> tombstones,
                                     JSONObject secrets) throws Exception {
        boolean changed = false;
        List<SshKeyEntry> local = JumpStore.loadKeys(context);
        JSONArray items = payload.optJSONArray("items");
        Set<String> remoteIds = new HashSet<>();
        if (items != null) {
            for (int i = 0; i < items.length(); i++) {
                JSONObject item = items.optJSONObject(i);
                if (item == null) continue;
                String id = item.optString("id", "");
                if (id.isEmpty()) continue;
                remoteIds.add(id);
                if (tombstones.contains("keys|" + id)) {
                    int before = local.size();
                    removeFromKeys(local, id);
                    if (local.size() != before) changed = true;
                    continue;
                }
                SshKeyEntry existing = JumpStore.findKey(local, id);
                SshKeyEntry incoming = keyFrom(item);
                long ms = BackupManager.msIso(item.optString("updatedAt", ""));
                boolean incomingNewer = existing == null || newer(ms, existing.updatedAt);
                if (!incomingNewer) continue;
                // 私钥材料：本机没有才写（有就保留本机 Keystore 密文）
                String pem = secrets.optString("key:" + id + ":private", "");
                if (existing != null && !existing.privateEnc.isEmpty()) {
                    incoming.privateEnc = existing.privateEnc;
                } else if (!pem.isEmpty()) {
                    incoming.privateEnc = KeyVault.encrypt(pem);
                }
                incoming.updatedAt = ms > 0 ? ms : System.currentTimeMillis();
                if (existing != null) {
                    local.set(local.indexOf(existing), incoming);
                } else {
                    local.add(incoming);
                }
                changed = true;
            }
        }
        for (SshKeyEntry entry : local) {
            if (!remoteIds.contains(entry.id) && !tombstones.contains("keys|" + entry.id)) {
                addTombstone(context, "keys", entry.id);
            }
        }
        if (changed) {
            JumpStore.saveKeys(context, local);
        }
        return changed;
    }

    private static SshKeyEntry keyFrom(JSONObject item) {
        SshKeyEntry entry = new SshKeyEntry();
        entry.id = item.optString("id", "");
        entry.name = item.optString("name", "");
        entry.algorithm = item.optString("algorithm", "");
        entry.fingerprint = item.optString("fingerprint", "");
        entry.publicKey = item.optString("publicKey", "");
        long created = BackupManager.msIso(item.optString("createdAt", ""));
        entry.createdAt = created > 0 ? created : System.currentTimeMillis();
        return entry;
    }

    /** LWW：remote 比本地新才算（0 = 极旧）；相等不更新 */
    private static boolean newer(long remote, long local) {
        return remote > local;
    }

    // ==================================================================
    // 墓碑
    // ==================================================================

    static void addTombstone(Context context, String cat, String id) {
        try {
            SharedPreferences prefs = ServerStore.prefs(context);
            JSONArray list = new JSONArray(prefs.getString("backup_sync_tombstones", "[]"));
            for (int i = 0; i < list.length(); i++) {
                JSONObject item = list.optJSONObject(i);
                if (item != null && cat.equals(item.optString("cat"))
                        && id.equals(item.optString("id"))) {
                    return;
                }
            }
            JSONObject item = new JSONObject();
            item.put("cat", cat);
            item.put("id", id);
            list.put(item);
            prefs.edit().putString("backup_sync_tombstones", list.toString()).apply();
        } catch (Exception ignored) {
        }
    }

    /** 同步成功后清理：墓碑 > 500 截半防膨胀 */
    private static void pruneTombstones(SharedPreferences prefs) {
        try {
            JSONArray list = new JSONArray(prefs.getString("backup_sync_tombstones", "[]"));
            if (list.length() <= 500) return;
            JSONArray half = new JSONArray();
            for (int i = list.length() / 2; i < list.length(); i++) {
                half.put(list.get(i));
            }
            prefs.edit().putString("backup_sync_tombstones", half.toString()).apply();
        } catch (Exception ignored) {
        }
    }

    private static void removeFromList(List<ServerProfile> list, String id) {
        for (int i = list.size() - 1; i >= 0; i--) {
            if (list.get(i).id.equals(id)) {
                list.remove(i);
            }
        }
    }

    private static void removeFromJumps(List<JumpServer> list, String id) {
        for (int i = list.size() - 1; i >= 0; i--) {
            if (list.get(i).id.equals(id)) {
                list.remove(i);
            }
        }
    }

    private static void removeFromKeys(List<SshKeyEntry> list, String id) {
        for (int i = list.size() - 1; i >= 0; i--) {
            if (list.get(i).id.equals(id)) {
                list.remove(i);
            }
        }
    }

    private static void post(Object callback, String error) {
        if (callback == null) return;
        MAIN.post(() -> {
            if (callback instanceof SyncCallback) {
                ((SyncCallback) callback).onDone(error);
            } else if (callback instanceof TestCallback) {
                ((TestCallback) callback).onDone("failed", error);
            }
        });
    }

    private static void post(TestCallback callback, String status, String error) {
        if (callback == null) return;
        MAIN.post(() -> callback.onDone(status, error));
    }
}
