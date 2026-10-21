package com.anyboot.remote;

import android.graphics.Color;
import android.net.Uri;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
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

    /** 四个电源动作（与 PC 端 hub.rs、hub.html 同一套字符串） */
    static final String POWER_WAKE = "wake";

    /** MAC 的常见写法：aa:bb:cc:dd:ee:ff / aa-bb-... / aabb.ccdd.eeff */
    static final java.util.regex.Pattern MAC_PATTERN = java.util.regex.Pattern.compile(
            "(?:[0-9a-fA-F]{2}[:-]){5}[0-9a-fA-F]{2}|[0-9a-fA-F]{4}\\.[0-9a-fA-F]{4}\\.[0-9a-fA-F]{4}");

    /** 每块网卡默认发几个唤醒包 */
    static final int DEFAULT_WAKE_BURST = 10;
    /** 唤醒包默认重试几次 */
    static final int DEFAULT_WAKE_RETRY = 10;
    /** 重启 / 关机 / 睡眠默认只发 1 个包 —— 同一个包到两次，sol 就执行两次 */
    static final int DEFAULT_POWER_BURST = 1;
    /** 重启 / 关机 / 睡眠默认重试 1 次（只在真没发出去时才补发） */
    static final int DEFAULT_POWER_RETRY = 1;
    static final String POWER_REBOOT = "reboot";
    static final String POWER_SHUTDOWN = "shutdown";
    static final String POWER_SLEEP = "sleep";

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
    /**
     * 每个地址对应这块网卡的 MAC（唤醒用）。
     *
     * <p>key = 归一化后的地址（host:port），value = 该网卡的 MAC 列表（小写冒号分隔）。
     * 一台电脑多块网卡时，同一行「地址 MAC MAC」可以跟多个，也可以分多行各写各的。
     * 只影响「唤醒」——连接、探活、隧道全都只用 {@link #addresses}，MAC 缺失不影响任何既有功能。
     */
    final Map<String, List<String>> macs = new LinkedHashMap<>();
    /**
     * 局域网电源路由：把「动作 → 广播地址 → 端口 → 用哪些 MAC」绑起来。
     *
     * <p>为什么要有：目标机上装的是 <b>sol</b>，它在局域网里按 UDP 端口区分动作
     * （9 唤醒、别的端口重启/关机/睡眠）。这里配的就是「发到哪、发到哪个端口、带哪块网卡」。
     *
     * <p>为空 = 没配，电源操作照旧：重启/关机/睡眠走电脑端的 {@code /api/power}，
     * 唤醒走标准 WOL（9 / 7 端口，有跳板机就由跳板机广播）。
     */
    final List<PowerRoute> powerRoutes = new ArrayList<>();

    /**
     * 一条电源路由。
     *
     * <p>字段可变是为了序列化方便；Compose 那边改完用 {@link #with} 换一个新对象，
     * 这样 mutableStateList 才会触发重组。
     */
    static final class PowerRoute {
        String action = POWER_WAKE;
        /** 广播地址（192.168.0.255 / 255.255.255.255）或单播 IP；空 = 自动 */
        String target = "";
        /** UDP 端口（0 = 9） */
        int port = 9;
        /** 用哪些网卡的 MAC；空 = 这台电脑配过的全部 */
        final List<String> macs = new ArrayList<>();
        boolean enabled = true;
        /** 每条路由每次发几个包（0 = 按动作默认：唤醒 10、其余 1）。每个动作都能设 */
        /** 这条路由每次发几个包（0 = 按动作默认：唤醒 10、其余 1） */
        int burst;
        /** 一轮没全发出去时重试几次（0 = 按动作默认：唤醒 10、其余 1） */
        int retry;
        /** sol 包没发出去时，改调这台电脑自带的 /api/power。所有动作默认勾上
         *（唤醒也能勾：HTTP 能送达就说明机器其实醒着，服务端直接回执） */
        boolean httpFallback;
        /**
         * 发送模式（跟在动作选择后面的单选）：
         * auto = 优先广播包（没发出去再走 HTTP，即旧 httpFallback=true 的语义）
         * http_first = 优先 HTTP（接口没通再发广播包）
         * http_only = 仅 HTTP（一个广播包都不发）
         * packet_only = 仅广播包（绝不调 HTTP）
         * 空 = 老数据，按 httpFallback 推导
         */
        String mode = "";

        /** 这条路由的发送模式（空 / 不认识按旧 httpFallback 推导，老档案平滑升级） */
        String effectiveMode() {
            if ("auto".equals(mode) || "http_first".equals(mode)
                    || "http_only".equals(mode) || "packet_only".equals(mode)
                    || "hybrid".equals(mode)) {
                return mode;
            }
            return httpFallback ? "auto" : "packet_only";
        }

        PowerRoute() {
        }

        PowerRoute(String action, String target, int port) {
            this.action = normalizePowerAction(action);
            this.target = text(target);
            this.port = port <= 0 ? 9 : port;
        }

        String actionOrWake() {
            return normalizePowerAction(action);
        }

        int effectivePort() {
            return port <= 0 ? 9 : port;
        }

        /** 每次发几个包（0 = 按动作默认，上限 50） */
        int effectiveBurst() {
            if (burst <= 0) {
                return defaultBurst();
            }
            return Math.min(50, burst);
        }

        /** 失败重试几次（0 = 按动作默认，上限 20） */
        int effectiveRetry() {
            if (retry <= 0) {
                return defaultRetry();
            }
            return Math.min(20, retry);
        }

        /** 没填发包数时的默认值：唤醒 10，其余 1 */
        int defaultBurst() {
            return ServerProfile.POWER_WAKE.equals(actionOrWake()) ? DEFAULT_WAKE_BURST : DEFAULT_POWER_BURST;
        }

        /** 没填重试次数时的默认值：唤醒 10，其余 1 */
        int defaultRetry() {
            return ServerProfile.POWER_WAKE.equals(actionOrWake()) ? DEFAULT_WAKE_RETRY : DEFAULT_POWER_RETRY;
        }

        PowerRoute copy() {
            PowerRoute other = new PowerRoute();
            other.action = actionOrWake();
            other.target = text(target);
            other.port = effectivePort();
            other.macs.addAll(macs);
            other.enabled = enabled;
            other.burst = burst;
            other.retry = retry;
            other.mode = mode;
            other.httpFallback = httpFallback;
            return other;
        }

        /** 换一个新实例（Compose 的列表靠替换元素触发重组） */
        PowerRoute with(String action, String target, int port, List<String> macs, boolean enabled) {
            PowerRoute other = new PowerRoute(action, target, port);
            other.macs.addAll(macs);
            other.enabled = enabled;
            other.burst = burst;
            other.retry = retry;
            other.mode = mode;
            other.httpFallback = httpFallback;
            return other;
        }

        /** 只改「发送模式」（其余字段照抄）；httpFallback 跟着派生，旧版兼容 */
        PowerRoute withMode(String value) {
            PowerRoute other = copy();
            other.mode = "auto".equals(value) || "http_first".equals(value)
                    || "http_only".equals(value) || "packet_only".equals(value) ? value : "";
            other.httpFallback = "auto".equals(other.effectiveMode())
                    || "http_first".equals(other.effectiveMode());
            return other;
        }

        /** 只改「发几个包 / 重试几次」（其余字段照抄） */
        PowerRoute withCounts(int burst, int retry) {
            PowerRoute other = with(action, target, port, macs, enabled);
            other.burst = burst <= 0 ? 0 : Math.min(50, burst);
            other.retry = retry <= 0 ? 0 : Math.min(20, retry);
            return other;
        }
    }
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
        pruneMacs();
    }

    /**
     * 用编辑框的完整解析结果替换（地址 + 每个地址的 MAC）。
     *
     * <p>MAC 以输入为准：这一行没写 MAC 就是没配（不是「沿用上次的」—— 用户删掉了
     * 就是不想再唤醒它）。地址归一化后作为 key，所以改端口会丢掉该地址的 MAC，
     * 这是有意的：端口变了就是另一条访问地址，旧 MAC 不再对应它。
     */
    void replaceParsed(Parsed parsed) {
        if (parsed == null) {
            replaceAddresses(new ArrayList<>());
            return;
        }
        replaceAddresses(parsed.hosts);
        macs.clear();
        for (Map.Entry<String, List<String>> entry : parsed.macs.entrySet()) {
            List<String> values = cleanMacs(entry.getValue());
            if (!values.isEmpty() && addresses.contains(entry.getKey())) {
                macs.put(entry.getKey(), values);
            }
        }
    }

    /** 把解析结果并进来（新增 / 合并同一台电脑时用）：只增不删，MAC 也一起并 */
    void mergeParsed(Parsed parsed) {
        if (parsed == null) {
            return;
        }
        for (String host : parsed.hosts) {
            addAddress(host);
        }
        for (Map.Entry<String, List<String>> entry : parsed.macs.entrySet()) {
            if (!addresses.contains(entry.getKey())) {
                continue;
            }
            List<String> mine = macs.get(entry.getKey());
            if (mine == null) {
                mine = new ArrayList<>();
                macs.put(entry.getKey(), mine);
            }
            for (String mac : cleanMacs(entry.getValue())) {
                if (!mine.contains(mac)) {
                    mine.add(mac);
                }
            }
        }
    }

    /** 该地址配的 MAC（没有返回空列表，调用方不用判空） */
    List<String> macsForHost(String host) {
        List<String> values = macs.get(normalizeHost(host));
        return values == null ? new ArrayList<>() : new ArrayList<>(values);
    }

    /** 第 index 个候选地址对应的 MAC */
    List<String> macsAt(int index) {
        return macsForHost(hostAt(index));
    }

    /** 这台电脑配过的所有 MAC（去重、保持地址顺序）—— 唤醒时全发一遍最省心 */
    List<String> allMacs() {
        List<String> result = new ArrayList<>();
        for (String address : addresses) {
            for (String mac : macsForHost(address)) {
                if (!result.contains(mac)) {
                    result.add(mac);
                }
            }
        }
        // 防御：macs 里可能残留已不在 addresses 中的键（手工改过 JSON 才会出现）
        for (List<String> values : macs.values()) {
            for (String mac : values) {
                if (!result.contains(mac)) {
                    result.add(mac);
                }
            }
        }
        return result;
    }

    boolean hasMacs() {
        return !allMacs().isEmpty();
    }

    /** 某个动作配了哪些（启用中的）局域网路由 */
    List<PowerRoute> routesFor(String action) {
        String wanted = normalizePowerAction(action);
        List<PowerRoute> result = new ArrayList<>();
        for (PowerRoute route : powerRoutes) {
            if (route.enabled && route.actionOrWake().equals(wanted)) {
                result.add(route);
            }
        }
        return result;
    }

    /**
     * 一个动作下所有路由**聚合**后的发送模式（与 PC 端 hub.rs routes_mode 同一套）：
     * 任何一条 hybrid → hybrid（同时发两条通道，覆盖面最大）；
     * 任何一条 http_first → http_first；全部 http_only → http_only（混进别的模式
     * 就按 auto，别把想发广播包的那条闷掉）；否则有 auto → auto；否则 packet_only。
     */
    static String routesMode(List<PowerRoute> routes) {
        if (routes == null || routes.isEmpty()) {
            return "auto";
        }
        boolean anyHybrid = false;
        boolean anyHttpFirst = false;
        boolean allHttpOnly = true;
        boolean anyAuto = false;
        for (PowerRoute route : routes) {
            String m = route.effectiveMode();
            if ("hybrid".equals(m)) {
                anyHybrid = true;
            } else if ("http_first".equals(m)) {
                anyHttpFirst = true;
            } else if ("auto".equals(m)) {
                anyAuto = true;
                allHttpOnly = false;
            } else if (!"http_only".equals(m)) {
                allHttpOnly = false;
            }
        }
        if (anyHybrid) {
            return "hybrid";
        }
        if (anyHttpFirst) {
            return "http_first";
        }
        if (allHttpOnly) {
            return "http_only";
        }
        return anyAuto ? "auto" : "packet_only";
    }

    /** 发送模式的展示文案（与 PC 端一致） */
    static String modeLabel(String mode) {
        if ("http_first".equals(mode)) {
            return "优先 HTTP";
        }
        if ("http_only".equals(mode)) {
            return "仅 HTTP";
        }
        if ("packet_only".equals(mode)) {
            return "仅广播包";
        }
        if ("hybrid".equals(mode)) {
            return "混合（同时发）";
        }
        return "优先广播包";
    }

    /** 这条路由实际要发的 MAC：路由里选了就用选的，没选 = 这台电脑配过的全部 */
    List<String> routeMacs(PowerRoute route) {
        if (route == null || route.macs.isEmpty()) {
            return allMacs();
        }
        return new ArrayList<>(route.macs);
    }

    /** 删掉不在地址列表里的 MAC（地址被用户删了，MAC 也留着没意义） */
    private void pruneMacs() {
        macs.keySet().removeIf(host -> !addresses.contains(host));
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
            // MAC 也一起并过来：同一台电脑两块网卡各配各的，合并时不能丢
            List<String> theirs = other.macsForHost(address);
            if (!theirs.isEmpty()) {
                List<String> mine = macs.get(address);
                if (mine == null) {
                    mine = new ArrayList<>();
                    macs.put(address, mine);
                }
                for (String mac : theirs) {
                    if (!mine.contains(mac)) {
                        mine.add(mac);
                    }
                }
            }
        }
        return added;
    }

    /** 编辑框内容：一行一个地址；该地址配了 MAC 就跟在同一行后面（可多个） */
    String addressText() {
        StringBuilder builder = new StringBuilder();
        for (String address : addresses) {
            if (builder.length() > 0) {
                builder.append('\n');
            }
            builder.append(address);
            for (String mac : macsForHost(address)) {
                builder.append(' ').append(mac);
            }
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
        for (Map.Entry<String, List<String>> entry : macs.entrySet()) {
            other.macs.put(entry.getKey(), new ArrayList<>(entry.getValue()));
        }
        other.token = token;
        other.machineId = machineId;
        other.machineName = machineName;
        other.probeMode = probeMode;
        other.jumpServerId = jumpServerId;
        for (PowerRoute route : powerRoutes) {
            other.powerRoutes.add(route.copy());
        }
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
        // MAC 按地址分组存：{ "192.168.0.120:12300": ["aa:bb:cc:dd:ee:ff"] }
        // 单独一个对象而不是把 MAC 塞进 addresses 字符串里 —— 旧版本读到 addresses
        // 只会看到纯地址，不会把 MAC 当访问地址去连（兼容 1.2 及更早）。
        JSONObject macObject = new JSONObject();
        for (Map.Entry<String, List<String>> entry : macs.entrySet()) {
            JSONArray values = new JSONArray();
            for (String mac : entry.getValue()) {
                values.put(mac);
            }
            if (values.length() > 0) {
                macObject.put(entry.getKey(), values);
            }
        }
        object.put("macs", macObject);
        // 局域网电源路由：动作 → 地址:端口 → MAC 子集
        JSONArray routeArray = new JSONArray();
        for (PowerRoute route : powerRoutes) {
            JSONObject item = new JSONObject();
            item.put("action", route.actionOrWake());
            item.put("target", text(route.target));
            item.put("port", route.effectivePort());
            JSONArray values = new JSONArray();
            for (String mac : route.macs) {
                values.put(mac);
            }
            item.put("macs", values);
            item.put("enabled", route.enabled);
            item.put("burst", route.burst <= 0 ? 0 : route.effectiveBurst());
            item.put("retry", route.retry <= 0 ? 0 : route.effectiveRetry());
            // mode 是权威值；httpFallback 按 mode 派生落盘（旧版客户端读新档案不反转）
            item.put("mode", route.effectiveMode());
            item.put("httpFallback", route.effectiveMode().equals("auto")
                    || route.effectiveMode().equals("http_first"));
            routeArray.put(item);
        }
        object.put("powerRoutes", routeArray);
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
        JSONObject macObject = object.optJSONObject("macs");
        if (macObject != null) {
            for (java.util.Iterator<String> keys = macObject.keys(); keys.hasNext(); ) {
                String rawHost = keys.next();
                String host = normalizeHost(rawHost);
                if (host.isEmpty()) {
                    continue;
                }
                JSONArray values = macObject.optJSONArray(rawHost);
                List<String> list = new ArrayList<>();
                if (values != null) {
                    for (int i = 0; i < values.length(); i++) {
                        list.add(values.optString(i, ""));
                    }
                }
                List<String> cleaned = cleanMacs(list);
                if (!cleaned.isEmpty()) {
                    profile.macs.put(host, cleaned);
                }
            }
            profile.pruneMacs();
        }
        profile.token = object.optString("token", "");
        profile.machineId = object.optString("machineId", "");
        profile.machineName = object.optString("machineName", "");
        // 只认两个值，其余（含缺失 / 拼错）一律回落到并发
        profile.probeMode = PROBE_SERIAL.equals(object.optString("probeMode", ""))
                ? PROBE_SERIAL : PROBE_PARALLEL;
        profile.jumpServerId = object.optString("jumpServerId", "");
        JSONArray routeArray = object.optJSONArray("powerRoutes");
        if (routeArray != null) {
            for (int i = 0; i < routeArray.length(); i++) {
                JSONObject item = routeArray.optJSONObject(i);
                if (item == null) {
                    continue;
                }
                PowerRoute route = new PowerRoute();
                route.action = normalizePowerAction(item.optString("action", ""));
                route.target = text(item.optString("target", ""));
                route.port = item.optInt("port", 9);
                if (route.port <= 0) {
                    route.port = 9;
                }
                route.enabled = item.optBoolean("enabled", true);
                route.burst = item.optInt("burst", 0);
                route.retry = item.optInt("retry", 0);
                route.httpFallback = item.optBoolean("httpFallback", true);
                route.mode = item.optString("mode", "");
                JSONArray values = item.optJSONArray("macs");
                if (values != null) {
                    for (int j = 0; j < values.length(); j++) {
                        String mac = normalizeMac(values.optString(j, ""));
                        if (!mac.isEmpty() && !route.macs.contains(mac)) {
                            route.macs.add(mac);
                        }
                    }
                }
                profile.powerRoutes.add(route);
            }
        }
        return profile;
    }

    // ------------------------------------------------------------------
    // 解析
    // ------------------------------------------------------------------

    /**
     * 解析地址输入框：**一行一个地址**（也兼容逗号 / 分号 / 空格分隔），
     * 同一台电脑的多块网卡可以一次全填进来。归一化 + 去重，保持输入顺序。
     *
     * <p>MAC 会被识别并跳过（不当地址）：只取地址的调用方用这个；
     * 要连 MAC 一起收就用 {@link #parseAddressInput(String)}。
     */
    static List<String> parseAddresses(String raw) {
        return parseAddressInput(raw).hosts;
    }

    /** {@link #parseAddressInput(String)} 的结果：地址列表 + 每个地址的 MAC */
    static final class Parsed {
        final List<String> hosts = new ArrayList<>();
        final Map<String, List<String>> macs = new LinkedHashMap<>();

        boolean isEmpty() {
            return hosts.isEmpty();
        }
    }

    /**
     * 解析地址输入框（带 MAC 的完整版）。
     *
     * <p>语法：**一行 = 一个地址，地址后面可以跟这块网卡的 MAC，可跟多个**：
     * <pre>
     * 192.168.0.120:12300 AA:BB:CC:DD:EE:FF
     * 192.168.0.121:12300 11-22-33-44-55-66 aabb.ccdd.eeff
     * 192.168.0.122:12300
     * </pre>
     *
     * <p>判定顺序（重要）：先认 IPv4 再认 MAC —— 否则 {@code 192.168.100.100}
     * 去掉点正好是 12 位十六进制，会被误当成 MAC。带端口的地址（含一个冒号）
     * 也不会命中「6 组两位」的 MAC 形态。
     *
     * <p>一行里出现多个非 MAC 的 token 时，它们各自都算地址（兼容老写法
     * 「一行填多个地址」）；MAC 归属于它前面最近的那个地址。整行只有 MAC 时
     * 归给上一行的地址（容错，不报错）。
     */
    static Parsed parseAddressInput(String raw) {
        return parseAddressInput(raw, false);
    }

    /**
     * 带 strict 的解析：{@code strict = true} 时「没点又没写端口的单词」不算地址
     * —— 快捷识别框会收到整段聊天记录 / Excel，里面的「NAS」「机柜」之类不是主机名。
     */
    static Parsed parseAddressInput(String raw, boolean strict) {
        Parsed parsed = new Parsed();
        if (raw == null) {
            return parsed;
        }
        String lastHost = "";
        for (String line : raw.split("\r?\n")) {
            String currentHost = "";
            for (String piece : line.split("[\\s,;\uFF0C\uFF1B]+")) {
                String token = piece.trim();
                if (token.isEmpty()) {
                    continue;
                }
                if (isMac(token)) {
                    String mac = normalizeMac(token);
                    String owner = currentHost.isEmpty() ? lastHost : currentHost;
                    if (mac.isEmpty() || owner.isEmpty()) {
                        continue;
                    }
                    List<String> list = parsed.macs.get(owner);
                    if (list == null) {
                        list = new ArrayList<>();
                        parsed.macs.put(owner, list);
                    }
                    if (!list.contains(mac)) {
                        list.add(mac);
                    }
                    continue;
                }
                String host = normalizeHost(token);
                if (host.isEmpty()) {
                    continue;
                }
                if (strict && !token.contains(":") && !host.contains(".")
                        && !host.toLowerCase(Locale.US).startsWith("localhost")) {
                    continue;
                }
                if (!parsed.hosts.contains(host)) {
                    parsed.hosts.add(host);
                }
                currentHost = host;
                lastHost = host;
            }
        }
        return parsed;
    }

    /**
     * 是不是一个 MAC 地址（支持 {@code aa:bb:cc:dd:ee:ff} / {@code aa-bb-...} /
     * {@code aabb.ccdd.eeff} / {@code aabbccddeeff}）。
     *
     * <p>IPv4 先排除：{@code 192.168.100.100} 去掉分隔符也是 12 位十六进制。
     */
    static boolean isMac(String raw) {
        String value = text(raw);
        if (value.isEmpty()) {
            return false;
        }
        if (value.matches("^(\\d{1,3}\\.){3}\\d{1,3}$")) {
            return false;
        }
        return normalizeMac(value).length() == 17;
    }

    /**
     * 动作归一化：英文 / 中文 / 旧别名都认，不认识的一律当「唤醒」。
     *
     * <p>为什么容错：这些值落进 servers.json，用户手工改过或从旧备份恢复的
     * 都可能有别名，宁可认出来也别让一条配好的路由悄悄失效。
     */
    static String normalizePowerAction(String raw) {
        String value = text(raw).toLowerCase(Locale.US);
        if (value.equals("wake") || value.equals("wol") || value.equals("wakeup")
                || value.equals("唤醒") || value.equals("开机")) {
            return POWER_WAKE;
        }
        if (value.equals("reboot") || value.equals("restart") || value.equals("重启")) {
            return POWER_REBOOT;
        }
        if (value.equals("shutdown") || value.equals("poweroff") || value.equals("power-off")
                || value.equals("关机")) {
            return POWER_SHUTDOWN;
        }
        if (value.equals("sleep") || value.equals("suspend") || value.equals("睡眠")) {
            return POWER_SLEEP;
        }
        return POWER_WAKE;
    }

    /** 动作的中文名（提示文案用） */
    static String powerActionLabel(String action) {
        switch (normalizePowerAction(action)) {
            case POWER_REBOOT:
                return "重启";
            case POWER_SHUTDOWN:
                return "关机";
            case POWER_SLEEP:
                return "睡眠";
            default:
                return "唤醒";
        }
    }

    /** 归一化成小写冒号分隔的 MAC；不是 MAC 返回空串 */
    static String normalizeMac(String raw) {
        String value = text(raw);
        if (value.isEmpty() || value.matches("^(\\d{1,3}\\.){3}\\d{1,3}$")) {
            return "";
        }
        // 从一坨文字里抠出 MAC：「11-22-33-44-55-66（5G）」这种粘着备注的也认
        java.util.regex.Matcher matcher = MAC_PATTERN.matcher(value);
        String source = matcher.find() ? matcher.group() : value;
        String hex = source.replace(":", "").replace("-", "").replace(".", "");
        if (hex.length() != 12 || !hex.matches("^[0-9a-fA-F]{12}$")) {
            return "";
        }
        StringBuilder builder = new StringBuilder();
        for (int i = 0; i < hex.length(); i += 2) {
            if (builder.length() > 0) {
                builder.append(':');
            }
            builder.append(hex.substring(i, i + 2).toLowerCase(Locale.US));
        }
        return builder.toString();
    }

    /** MAC 列表归一化 + 去重（保持顺序） */
    private static List<String> cleanMacs(List<String> values) {
        List<String> result = new ArrayList<>();
        if (values == null) {
            return result;
        }
        for (String value : values) {
            String mac = normalizeMac(value);
            if (!mac.isEmpty() && !result.contains(mac)) {
                result.add(mac);
            }
        }
        return result;
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
