package com.anyboot.remote;

import android.graphics.Color;
import android.net.Uri;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;
import java.util.UUID;

/**
 * 一条记录 = **一台电脑**（不是一个 IP）。
 *
 * <p>一台电脑可能有多块网卡、或者 DHCP 每次给不同的 IP —— 这些地址其实是同一台机器，
 * 所以这里存的是一个**有序候选地址列表** {@link #addresses}：
 * <ul>
 *   <li>第 0 个是首选地址（上次连上的那个会被自动提到最前，下次直连）；</li>
 *   <li>加载失败就换下一个地址，但**还是同一个标签、同一个 WebView** ——
 *       一台电脑永远只占一份页面，不会「一个网卡一个标签」；</li>
 *   <li>候选地址全试完才认为这台电脑连不上。</li>
 * </ul>
 *
 * <p>「是不是同一台电脑」优先用电脑端 `GET /health` 给的 {@link #machineId} 判断
 * （{@link #sameMachine}）；拿不到标识时退回「分组名相同」这个用户可见的约定。
 */
final class ServerProfile {

    /** 电脑端「远程控制」的默认端口 */
    static final int DEFAULT_PORT = 12300;

    /** 多网卡探测：并发（默认）—— 同探所有地址取延迟最低 */
    static final String PROBE_PARALLEL = "parallel";
    /** 多网卡探测：串行 —— 按地址顺序逐个探，第一个通的用之 */
    static final String PROBE_SERIAL = "serial";

    String id;
    /** 分组名（用来区分不同的电脑）；空串表示「未分组」 */
    String group;
    /** 服务端名；空串表示用地址兜底 */
    String name;
    /**
     * 候选地址（有序，第 0 个是首选）。**唯一事实来源**，永不为空 ——
     * 一个地址都没有的条目在 {@link ServerStore#load} 里就被丢掉了。
     */
    final List<String> addresses = new ArrayList<>();
    /** 访问令牌；可空 */
    String token;
    /** 电脑端 `/health` 给的机器标识（同一台电脑的多个 IP 靠它归并）；未知为空 */
    String machineId = "";
    /** 电脑端 `/health` 给的机器名（「这是哪台电脑」）；未知为空 */
    String machineName = "";
    /** 多网卡探测模式：{@link #PROBE_PARALLEL}（默认）/ {@link #PROBE_SERIAL} */
    String probeMode = PROBE_PARALLEL;
    /** 经由哪台跳板机（JumpServer.id）；空 = 直连，行为与现在完全一致。
     *  走跳板机时 addresses 语义变为「跳板机视角的内网地址」。 */
    String jumpServerId = "";

    ServerProfile() {
        this.id = UUID.randomUUID().toString();
        this.group = "";
        this.name = "";
        this.token = "";
    }

    // ------------------------------------------------------------------
    // 地址
    // ------------------------------------------------------------------

    /** 首选地址（没有地址时是空串） */
    String host() {
        return addresses.isEmpty() ? "" : addresses.get(0);
    }

    /** 第 index 个候选地址（越界返回空串，调用方不用自己判边界） */
    String hostAt(int index) {
        return index >= 0 && index < addresses.size() ? addresses.get(index) : "";
    }

    int addressCount() {
        return addresses.size();
    }

    /** 是否已经有这个地址（归一化之后比较） */
    boolean hasAddress(String host) {
        String value = normalizeHost(host);
        return !value.isEmpty() && addresses.contains(value);
    }

    /** 追加一个地址（归一化 + 去重）。返回是否真的加进去了。 */
    boolean addAddress(String host) {
        String value = normalizeHost(host);
        if (value.isEmpty() || addresses.contains(value)) {
            return false;
        }
        addresses.add(value);
        return true;
    }

    /** 用用户输入替换地址列表（编辑入口：以输入的顺序为准） */
    void replaceAddresses(List<String> hosts) {
        addresses.clear();
        addresses.addAll(clean(hosts));
    }

    /**
     * 把这个地址提到最前（下次优先用它 —— 刚连通的那个地址才是「这台电脑现在的地址」）。
     * 本来就是首选时返回 false。
     */
    boolean promoteAddress(int index) {
        if (index <= 0 || index >= addresses.size()) {
            return false;
        }
        addresses.add(0, addresses.remove(index));
        return true;
    }

    /** 并入另一条记录的地址（去重，本条已有的顺序不变），返回新增个数 */
    int mergeAddresses(ServerProfile other) {
        if (other == null) {
            return 0;
        }
        int added = 0;
        for (String address : other.addresses) {
            if (addAddress(address)) {
                added++;
            }
        }
        return added;
    }

    /** 编辑框内容：一行一个地址 */
    String addressText() {
        StringBuilder builder = new StringBuilder();
        for (String address : addresses) {
            if (builder.length() > 0) {
                builder.append('\n');
            }
            builder.append(address);
        }
        return builder.toString();
    }

    /** 列表 / 错误面板里显示的地址摘要：首选地址（还有备用时补一句） */
    String addressSummary() {
        String primary = host();
        int spare = addresses.size() - 1;
        return spare > 0 ? primary + "（另有 " + spare + " 个地址）" : primary;
    }

    /** 带 token 的访问地址（token 会做百分号编码） */
    String url() {
        return urlAt(0);
    }

    /** 第 index 个候选地址的访问地址 */
    String urlAt(int index) {
        return urlAt(index, "");
    }

    /**
     * 第 index 个候选地址的访问地址，并附带传输方式偏好（空 / auto 时不加）。
     *
     * <p>为什么这个参数要由 App 从 URL 上带过去：网页端的 localStorage 是按
     * **来源站点**存的，一台电脑有多块网卡时会换地址（换地址 = 换来源），
     * 偏好就跟着丢了。App 每次加载都带上，换地址也不会丢。
     */
    String urlAt(int index, String transport) {
        String host = hostAt(index);
        if (host.isEmpty()) {
            return "";
        }
        StringBuilder url = new StringBuilder("http://").append(host).append("/");
        String value = text(token);
        if (!value.isEmpty()) {
            url.append("?token=").append(Uri.encode(value));
        }
        String mode = text(transport);
        if (!mode.isEmpty() && !"auto".equals(mode)) {
            url.append(value.isEmpty() ? "?" : "&").append("transport=").append(mode);
        }
        return url.toString();
    }

    // ------------------------------------------------------------------
    // 访问地址
    // ------------------------------------------------------------------

    /**
     * 用给定的 host:port 造访问地址（token / transport 照常带上）。
     *
     * <p>走跳板机的电脑：主机不是 addresses 里那个，而是隧道解析出的
     * {@code 127.0.0.1:本地端口} —— 页面照常从 localhost 加载，所有请求
     * 都经 SSH 加密通道 → 跳板机 → 内网 anyboot。
     */
    String urlFor(String hostPort, String transport) {
        String value = text(hostPort);
        if (value.isEmpty()) {
            return "";
        }
        StringBuilder url = new StringBuilder("http://").append(value).append("/");
        String secret = text(token);
        if (!secret.isEmpty()) {
            url.append("?token=").append(Uri.encode(secret));
        }
        String mode = text(transport);
        if (!mode.isEmpty() && !"auto".equals(mode)) {
            url.append(secret.isEmpty() ? "?" : "&").append("transport=").append(mode);
        }
        return url.toString();
    }

    // ------------------------------------------------------------------
    // 展示
    // ------------------------------------------------------------------

    /** 标签上显示的服务端名（没起名字就用地址） */
    String displayName() {
        String value = text(name);
        return value.isEmpty() ? host() : value;
    }

    /** 标签文本：组名 + 服务端名（未分组时只显示服务端名） */
    String label() {
        String groupName = text(group);
        return groupName.isEmpty() ? displayName() : groupName + " · " + displayName();
    }

    /** 排序键：先按分组（未分组排最后），再按显示名 */
    String sortKey() {
        String groupName = text(group);
        String prefix = groupName.isEmpty() ? "\uffff" : groupName;
        return prefix + "\u0000" + displayName() + "\u0000" + host();
    }

    /**
     * 分组配色：同一个分组恒定同一个颜色（取组名的哈希定色相），
     * 让人扫一眼标签栏就能看出「哪几个标签是同一台电脑」。
     */
    int groupColor() {
        String groupName = text(group);
        if (groupName.isEmpty()) {
            return 0xFFB9BFC9;
        }
        int hash = 0;
        for (int i = 0; i < groupName.length(); i++) {
            hash = hash * 31 + groupName.charAt(i);
        }
        float hue = Math.abs(hash % 360);
        return Color.HSVToColor(new float[] { hue, 0.45f, 0.78f });
    }

    // ------------------------------------------------------------------
    // 「是不是同一台电脑」
    // ------------------------------------------------------------------

    /**
     * 是不是同一台电脑（可以合并成一条记录）。
     *
     * <p>优先用电脑端的机器标识比对：两边都拿到了 {@link #machineId} 时以标识为准
     * （标识不同就是不同电脑，哪怕分组名写一样）。任一边拿不到标识时，退回
     * 「分组名相同」这个用户可见的约定 —— 分组本来就是「一台电脑」的意思。
     */
    boolean sameMachine(ServerProfile other) {
        if (other == null || other == this || id.equals(other.id)) {
            return false;
        }
        String mine = text(machineId);
        String theirs = text(other.machineId);
        if (!mine.isEmpty() && !theirs.isEmpty()) {
            return mine.equalsIgnoreCase(theirs);
        }
        String myGroup = text(group);
        String otherGroup = text(other.group);
        return !myGroup.isEmpty() && myGroup.equalsIgnoreCase(otherGroup);
    }

    ServerProfile copy() {
        ServerProfile other = new ServerProfile();
        other.id = id;
        other.group = group;
        other.name = name;
        other.addresses.addAll(addresses);
        other.token = token;
        other.machineId = machineId;
        other.machineName = machineName;
        other.probeMode = probeMode;
        other.jumpServerId = jumpServerId;
        return other;
    }

    // ------------------------------------------------------------------
    // 序列化
    // ------------------------------------------------------------------

    JSONObject toJson() throws JSONException {
        JSONObject object = new JSONObject();
        object.put("id", id);
        object.put("group", text(group));
        object.put("name", text(name));
        // host = 首选地址：1.1 及更早的客户端读的是这个键，继续写出去，
        // 旧版/旧备份仍然读得懂；新版本读的是 addresses（完整候选列表）。
        object.put("host", host());
        JSONArray array = new JSONArray();
        for (String address : addresses) {
            array.put(address);
        }
        object.put("addresses", array);
        object.put("token", text(token));
        object.put("machineId", text(machineId));
        object.put("machineName", text(machineName));
        object.put("probeMode", text(probeMode).isEmpty() ? PROBE_PARALLEL : probeMode);
        object.put("jumpServerId", text(jumpServerId));
        return object;
    }

    static ServerProfile fromJson(JSONObject object) {
        ServerProfile profile = new ServerProfile();
        String id = object.optString("id", "");
        if (!id.isEmpty()) {
            profile.id = id;
        }
        profile.group = object.optString("group", "");
        profile.name = object.optString("name", "");
        JSONArray array = object.optJSONArray("addresses");
        if (array != null) {
            for (int i = 0; i < array.length(); i++) {
                profile.addAddress(array.optString(i, ""));
            }
        }
        if (profile.addresses.isEmpty()) {
            // 1.1 及更早：整个条目只有一个 host
            profile.addAddress(object.optString("host", ""));
        }
        profile.token = object.optString("token", "");
        profile.machineId = object.optString("machineId", "");
        profile.machineName = object.optString("machineName", "");
        // 只认两个值，其余（含缺失 / 拼错）一律回落到并发
        profile.probeMode = PROBE_SERIAL.equals(object.optString("probeMode", ""))
                ? PROBE_SERIAL : PROBE_PARALLEL;
        profile.jumpServerId = object.optString("jumpServerId", "");
        return profile;
    }

    // ------------------------------------------------------------------
    // 解析
    // ------------------------------------------------------------------

    /**
     * 解析地址输入框：**一行一个地址**（也兼容逗号 / 分号 / 空格分隔），
     * 同一台电脑的多块网卡可以一次全填进来。归一化 + 去重，保持输入顺序。
     */
    static List<String> parseAddresses(String raw) {
        List<String> pieces = new ArrayList<>();
        if (raw != null) {
            for (String piece : raw.split("[\\r\\n,;\uFF0C\uFF1B\\s]+")) {
                pieces.add(piece);
            }
        }
        return clean(pieces);
    }

    /** 归一化 + 去掉空项 + 去重（保持顺序） */
    private static List<String> clean(List<String> hosts) {
        List<String> result = new ArrayList<>();
        for (String host : hosts) {
            String value = normalizeHost(host);
            if (!value.isEmpty() && !result.contains(value)) {
                result.add(value);
            }
        }
        return result;
    }

    /**
     * 用户可能粘贴整条 URL（http://192.168.0.120:12300/?token=xxx），
     * 也可能只写 IP，统一归一成 host:port。
     */
    static String normalizeHost(String raw) {
        String value = text(raw);
        if (value.isEmpty()) {
            return "";
        }
        int scheme = value.indexOf("://");
        if (scheme >= 0) {
            value = value.substring(scheme + 3);
        }
        int slash = value.indexOf('/');
        if (slash >= 0) {
            value = value.substring(0, slash);
        }
        int question = value.indexOf('?');
        if (question >= 0) {
            value = value.substring(0, question);
        }
        value = value.trim();
        if (value.isEmpty()) {
            return "";
        }
        if (!value.contains(":")) {
            value = value + ":" + DEFAULT_PORT;
        }
        return value;
    }

    /** 从粘贴的 URL 里顺手把 token 挖出来。 */
    static String tokenFromUrl(String raw) {
        if (raw == null || raw.indexOf("token=") < 0) {
            return "";
        }
        try {
            String token = Uri.parse(raw.trim()).getQueryParameter("token");
            return token == null ? "" : token.trim();
        } catch (Exception ignored) {
            return "";
        }
    }

    /** 去首尾空白（所有可空字符串字段统一走这里，免得满地判 null） */
    static String text(String raw) {
        return raw == null ? "" : raw.trim();
    }
}
