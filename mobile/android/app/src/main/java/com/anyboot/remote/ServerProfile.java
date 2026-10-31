package com.anyboot.remote;

import android.graphics.Color;
import android.net.Uri;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
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

    /** 电源包默认发几个（每条路由、每块网卡）。2026-10-31 起所有动作统一默认 10 */
    static final int DEFAULT_WAKE_BURST = 10;
    /** 电源包默认重试几次；所有动作统一 */
    static final int DEFAULT_WAKE_RETRY = 10;
    static final String POWER_REBOOT = "reboot";
    static final String POWER_SHUTDOWN = "shutdown";
    static final String POWER_SLEEP = "sleep";

    /** 多网卡探测：并发（首个）—— 每地址一个线程同探，任一响应立即胜出，其余作废 */
    static final String PROBE_PARALLEL_FIRST = "parallel_first";
    /** 多网卡探测：并发（全部）—— 等全部地址探完，逐行报成败与耗时，选最快的成功者 */
    static final String PROBE_PARALLEL_ALL = "parallel_all";
    /** 多网卡探测：串行 —— 按地址顺序逐个探，第一个通的用之 */
    static final String PROBE_SERIAL = "serial";
    /** 旧值兼容：parallel = 并发（首个） */
    static final String PROBE_PARALLEL = PROBE_PARALLEL_FIRST;

    /** 探测超时默认值（毫秒）：局域网 RTT 通常 <10ms，2 秒足够；公网可在连接策略里调大 */
    static final int DEFAULT_PROBE_TIMEOUT_MS = 2000;
    /** 探测超时合法区间：太小会把正常的公网握手判死，太大等于卡界面 */
    static final int MIN_PROBE_TIMEOUT_MS = 200;
    static final int MAX_PROBE_TIMEOUT_MS = 30000;

    /** 探测超时归一：0/负数 → 默认 2 秒；越界夹到 [200ms, 30s]（与 PC 端 clamp_probe_timeout_ms 一致） */
    static int clampProbeTimeoutMs(int raw) {
        if (raw <= 0) {
            return DEFAULT_PROBE_TIMEOUT_MS;
        }
        return Math.max(MIN_PROBE_TIMEOUT_MS, Math.min(MAX_PROBE_TIMEOUT_MS, raw));
    }

    /**
     * 每通道超时表的清洗：非法通道 id 丢掉；0 视作「没单独设」直接删（回落默认）；
     * 越界值夹到 [200ms, 30s]（与 PC 端 normalize_route_timeouts 同语义）。
     */
    static Map<String, Integer> normalizeRouteTimeouts(Map<String, Integer> raw) {
        Map<String, Integer> out = new LinkedHashMap<>();
        if (raw == null) {
            return out;
        }
        for (Map.Entry<String, Integer> entry : raw.entrySet()) {
            String key = text(entry.getKey()).toLowerCase(Locale.US);
            Integer value = entry.getValue();
            if (value == null || value <= 0 || !isRouteKind(key)) {
                continue;
            }
            out.put(key, clampProbeTimeoutMs(value));
        }
        return out;
    }

    /** 系统族归一：只认三个徽标值（windows / macos / linux），其余归空（不画徽标） */
    static String normalizeMachineOs(String raw) {
        String value = text(raw).toLowerCase(Locale.US);
        if (value.equals("windows")) {
            return "windows";
        }
        if (value.equals("macos") || value.equals("darwin") || value.equals("osx")) {
            return "macos";
        }
        if (value.equals("linux")) {
            return "linux";
        }
        return "";
    }

    /** 探测模式归一：serial / parallel_all 之外（含旧 parallel、空值）一律并发（首个） */
    static String normalizeProbeMode(String raw) {
        String value = text(raw);
        if (value.equals(PROBE_SERIAL)) {
            return PROBE_SERIAL;
        }
        if (value.equals(PROBE_PARALLEL_ALL)) {
            return PROBE_PARALLEL_ALL;
        }
        return PROBE_PARALLEL_FIRST;
    }

    /**
     * 连接优先级探测方式归一：三种合法值原样保留，空值 / 拼错的归**默认并发全部**。
     * 与 {@link #normalizeProbeMode}（多网卡粒度，默认并发首个）是两个口径，别混用。
     */
    static String normalizeRouteProbeMode(String raw) {
        String value = text(raw);
        if (value.equals(PROBE_SERIAL) || value.equals(PROBE_PARALLEL_ALL)
                || value.equals(PROBE_PARALLEL_FIRST)) {
            return value;
        }
        return PROBE_PARALLEL_ALL;
    }

    String id;
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
        /** 每条路由每次发几个包（0 = 默认 10，所有动作一样，2026-10-31）。每个动作都能设 */
        /** 这条路由每次发几个包（0 = 默认 10，所有动作一样） */
        int burst;
        /** 一轮没全发出去时重试几次（0 = 默认 10，所有动作一样） */
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

        /** 没填发包数时的默认值：所有动作统一 10（2026-10-31 起不再只有唤醒 10） */
        int defaultBurst() {
            return DEFAULT_WAKE_BURST;
        }

        /** 没填重试次数时的默认值：所有动作统一 10 */
        int defaultRetry() {
            return DEFAULT_WAKE_RETRY;
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
    /** 电脑端 `/health` 给的系统族：windows / macos / linux；未知为空（客户端据此画系统徽标） */
    String machineOs = "";
    /**
     * **多网卡探测**（同一通道内的多个地址之间怎么探）：
     * {@link #PROBE_PARALLEL_FIRST}（默认）/ {@link #PROBE_PARALLEL_ALL} / {@link #PROBE_SERIAL}。
     * <p>注意与 {@link #routeProbeMode} 是两个粒度：这个只管「一台电脑的多块网卡 / 多个 IP」。
     */
    String probeMode = PROBE_PARALLEL;
    /**
     * **连接优先级的探测方式**（六个通道之间怎么探，作用于所有通道元素）：
     * parallel_all（默认，等全部返回，逐行报）/ parallel_first（全部通道同时发出、谁先成功用谁）/
     * serial（按优先级逐个通道试，第一个通就停）。
     * <p>与 {@link #probeMode} 组合成两层：外层决定通道之间，内层决定同一通道内多个地址。
     */
    String routeProbeMode = PROBE_PARALLEL_ALL;
    /**
     * **默认**探测超时（毫秒，默认 2000）：某个通道没单独设时用它。
     * 0 = 未设置（用默认值）；实际使用前过 {@link #clampProbeTimeoutMs(int)}。
     */
    int probeTimeoutMs = DEFAULT_PROBE_TIMEOUT_MS;
    /**
     * **每个通道自己的探测超时**（通道 id → 毫秒）。缺 key（或 0）= 该通道用上面那个默认值。
     *
     * <p>为什么要分开：局域网 RTT 通常 &lt;10ms，2 秒都算宽；而 Funnel / 内置 tsnet
     * 走中继时首次 TLS 握手可能好几秒 —— 一个全局值必然一边偏松一边偏紧。
     */
    Map<String, Integer> routeTimeoutsMs = new LinkedHashMap<>();
    /** 经由哪台跳板机（JumpServer.id）；空 = 直连，行为与现在完全一致。
     *  走跳板机时 addresses 语义变为「跳板机视角的内网地址」。 */
    String jumpServerId = "";
    /**
     * 远程接入配对（多条目）：三个区块（cloudflare / ts / tsnet）各自可以挂多条，
     * 全部保留、全部独立探测。取代原来的四个单值槽 + 单值令牌
     * （{@code docs/remote-pairs.md} §4.1）。
     */
    /**
     * 每个档案的配对数软上限（与 PC 端 `MAX_PAIRS` 同一个值）：超过时 UI 拒绝新增并提示。
     * 不设上限的话「一张码一条」久了会堆出几百条，探测就变成 DDoS 自己。
     */
    static final int MAX_PAIRS = 32;

    final List<RemotePair> pairs = new ArrayList<>();    /**
     * **区块内部**的探测策略（区块 id → parallel_first / parallel_all / serial）。
     * 缺 key = 默认并发（首个）；与外层 {@link #routeProbeMode}（区块之间）组合成两层。
     *
     * <p>为什么不并入 {@link #probeMode}：那个管的是同一通道内多个局域网地址，
     * 地址是自动收集、随 DHCP 漂移的；配对是手填、稳定、按条目禁用。合成一个字段会让
     * 「DHCP 换了 IP 该替换还是新增」失去判断依据。
     */
    final Map<String, String> blockProbeMode = new LinkedHashMap<>();
    /** **区块默认**探测超时（区块 id → 毫秒）。缺 key（或 0）= 用全局默认值。 */
    final Map<String, Integer> blockTimeoutsMs = new LinkedHashMap<>();
    /**
     * 连接优先：{@code remote_first} = 远程优先（隧道通了就走隧道，局域网只做兜底）；
     * 空 / 其它 = 自动（局域网优先，远程兜底 —— 默认）。
     * <p><b>已废弃</b>：被 {@link #routeOrder}（五通道显式排序）取代，字段保留只为旧档案
     * 兼容（读取时迁移：remote_first → 远程通道在前、LAN 殿后），不再被消费逻辑读取。
     */
    String connMode = "";

    /** 云同步 LWW 的依据（毫秒时间戳）；编辑保存时刷新，探测不刷 */
    long updatedAt = 0;

    /** 连接优先级五通道 id */
    static final String ROUTE_LAN = "lan";
    static final String ROUTE_JUMP = "jump";
    static final String ROUTE_TUNNEL = "tunnel";
    static final String ROUTE_TS = "ts";
    static final String ROUTE_FUNNEL = "funnel";
    static final String ROUTE_TSNET = "tsnet";

    /** 连接优先级（六通道并列，有序）；空 = 老档案，按 connMode 迁移或默认序 */
    final List<String> routeOrder = new ArrayList<>();
    /**
     * 禁用的通道 id（认全部六个：lan / jump / tunnel / ts / funnel / tsnet）。
     *
     * <p>与 PC 端 `route_disabled` 同语义（2026-10-28 修复对齐）：连接策略表每行
     * 都有启停开关，区块通道的停用也落在这里 —— 以前只认 lan / jump，用户在
     * 电脑列表关掉 Cloudflare/Tailscale 通道保存后，重读档案开关就自己弹回来，
     * 探测连接照样走它。配对条目自己的开关仍走 {@link RemotePair#enabled}
     * （区块里三条配对想只停第二条，「整块禁用」表达不了），两层互不覆盖。
     */
    final List<String> routeDisabled = new ArrayList<>();

    /**
     * 「是否展示」被关掉的通道（2026-10-31 改口径，原「是否展示连通性」，与 PC 端
     * route_probe_hidden 同语义）：电脑列表**整行不展示**这些通道条目（连通性、
     * 网址、来源、复制/打开全不显示）。探测照跑、连接选择照常 —— 纯展示开关，
     * 不进连接判定（{@link #sameConnState} 也不比对它，改开关不触发重连）。
     * 与配对条目自己的 {@link RemotePair#showProbe} 两层任一命中即整行隐藏。
     */
    final List<String> routeProbeHidden = new ArrayList<>();

    /** 这条配对要不要在电脑列表展示（两层开关任一关掉即整行隐藏） */
    boolean probeHidden(RemotePair pair) {
        return pair != null && (!pair.showProbe || routeProbeHidden.contains(pair.routeKind()));
    }

    /** 默认优先级：直连 → 跳板 → 隧道 → Tailscale 直连 → Funnel → 内置 tsnet（越往后越绕） */
    static List<String> defaultRouteOrder() {
        return new ArrayList<>(Arrays.asList(
                ROUTE_LAN, ROUTE_JUMP, ROUTE_TUNNEL, ROUTE_TS, ROUTE_FUNNEL, ROUTE_TSNET));
    }

    /** 合法通道 id（读写档案时的白名单；加通道只改这一处 + defaultRouteOrder） */
    static boolean isRouteKind(String id) {
        return ROUTE_LAN.equals(id) || ROUTE_JUMP.equals(id) || ROUTE_TUNNEL.equals(id)
                || ROUTE_TS.equals(id) || ROUTE_FUNNEL.equals(id) || ROUTE_TSNET.equals(id);
    }

    /** 默认探测超时（毫秒，通道没单独设时用它） */
    int defaultProbeTimeoutMs() {
        return clampProbeTimeoutMs(probeTimeoutMs);
    }

    /**
     * 某个通道自己的探测超时（毫秒）：单独设过就用它，否则回落默认值。
     *
     * <p>与 PC 端 `RemoteServer::probe_timeout_for` 同语义 —— 局域网给 1~2 秒、
     * 走公网中继的 Funnel / 内置 tsnet 单独给 5~8 秒，各算各的。
     */
    int probeTimeoutFor(String kind) {
        Integer own = routeTimeoutsMs == null ? null : routeTimeoutsMs.get(text(kind).toLowerCase(Locale.US));
        if (own != null && own > 0) {
            return clampProbeTimeoutMs(own);
        }
        return defaultProbeTimeoutMs();
    }

    /** Funnel 判型：https + 域名含 .ts.net（与 Tailscale 100.x 直连区分；与 PC is_funnel_origin 同语义） */
    static boolean isFunnelOrigin(String origin) {
        String lower = text(origin).toLowerCase();
        return lower.startsWith("https://") && lower.substring(8).contains(".ts.net");
    }

    // ------------------------------------------------------------------
    // 配对区块（三个区块 = 存储层；六个通道 = 上报/展示层）
    // ------------------------------------------------------------------

    /**
     * 区块顺序：从 {@link #routeOrderSorted()} 里的配对通道推导（去重保序），
     * 缺的按默认序补尾。
     *
     * <p>为什么不新增排序字段：区块顺序本来就是用户对通道的排序，`ts` 与 `funnel`
     * 同属 ts 区块，取先出现者定位即可（{@code docs/remote-pairs.md} §5.1）。
     */
    List<String> pairBlockOrder() {
        LinkedHashSet<String> seen = new LinkedHashSet<>();
        for (String kind : routeOrderSorted()) {
            String block = RemotePair.blockOfRoute(kind);
            if (!block.isEmpty()) {
                seen.add(block);
            }
        }
        for (String block : RemotePair.BLOCK_IDS) {
            seen.add(block);
        }
        return new ArrayList<>(seen);
    }

    /** 某个区块里的配对（按数组顺序；{@code enabledOnly} = 跳过停用的） */
    List<RemotePair> pairsOfBlock(String block, boolean enabledOnly) {
        List<RemotePair> out = new ArrayList<>();
        String target = text(block).toLowerCase(Locale.US);
        for (RemotePair pair : pairs) {
            if (pair == null || !target.equals(text(pair.kind).toLowerCase(Locale.US))) {
                continue;
            }
            if (enabledOnly && !pair.enabled) {
                continue;
            }
            if (text(pair.addr).isEmpty()) {
                continue;
            }
            out.add(pair);
        }
        return out;
    }

    /** 按区块顺序展开所有可用配对（前 1/2 通道的扁平视图） */
    List<RemotePair> flatPairs() {
        List<RemotePair> out = new ArrayList<>();
        for (String block : pairBlockOrder()) {
            out.addAll(pairsOfBlock(block, true));
        }
        return out;
    }

    /** 生效配对：区块顺序 + 区块内顺序，第一条启用的；没有返回 null */
    RemotePair effectivePair() {
        List<RemotePair> flat = flatPairs();
        return flat.isEmpty() ? null : flat.get(0);
    }

    /** 生效地址（不再是存储字段：单值镜像会让「同区块第二条」无处表达） */
    String effectiveRemoteAddr() {
        RemotePair pair = effectivePair();
        return pair == null ? "" : normalizeRemoteOrigin(pair.addr);
    }

    /** 生效令牌（= 赢家那条自己的令牌） */
    String effectiveRemoteToken() {
        RemotePair pair = effectivePair();
        return pair == null ? "" : text(pair.token);
    }

    /**
     * 这条配对的探测超时：条目自己的 → 区块默认 → 全局默认（三层继承）。
     * 与 PC 端 `probe_timeout_for_pair` 同语义。
     */
    int probeTimeoutForPair(RemotePair pair) {
        if (pair != null && pair.probeTimeoutMs > 0) {
            return clampProbeTimeoutMs(pair.probeTimeoutMs);
        }
        String block = pair == null ? "" : text(pair.kind).toLowerCase(Locale.US);
        Integer blockTimeout = blockTimeoutsMs == null ? null : blockTimeoutsMs.get(block);
        if (blockTimeout != null && blockTimeout > 0) {
            return clampProbeTimeoutMs(blockTimeout);
        }
        return defaultProbeTimeoutMs();
    }

    /** 区块内部的探测策略（缺 key = 并发首个，与 PC 端同默认值） */
    String blockProbeModeFor(String block) {
        String value = blockProbeMode == null ? "" : text(blockProbeMode.get(text(block).toLowerCase(Locale.US)));
        if (PROBE_SERIAL.equals(value) || PROBE_PARALLEL_ALL.equals(value)) {
            return value;
        }
        return PROBE_PARALLEL_FIRST;
    }

    /** 一个配对展开出的连接候选项（令牌与超时在展开时就按条目算好，消费方不必重复实现继承） */
    static final class RouteCandidate {
        final String kind;
        final String origin;
        final String token;
        final int timeoutMs;
        final String pairId;

        RouteCandidate(String kind, String origin, String token, int timeoutMs, String pairId) {
            this.kind = kind;
            this.origin = origin;
            this.token = token;
            this.timeoutMs = timeoutMs;
            this.pairId = pairId;
        }

        boolean isRemote() {
            return !ServerProfile.ROUTE_LAN.equals(kind) && !ServerProfile.ROUTE_JUMP.equals(kind);
        }
    }

    /** 有序六通道（自愈）：清洗非法 id / 去重，缺的按默认序补尾；全空按 connMode 迁移 */
    List<String> routeOrderSorted() {
        LinkedHashSet<String> seen = new LinkedHashSet<>();
        for (String raw : routeOrder) {
            String id = text(raw);
            if (isRouteKind(id)) {
                seen.add(id);
            }
        }
        if (seen.isEmpty() && isRemoteFirst()) {
            // 迁移：远程通道在前（保持 隧道→ts→funnel 的相对序），跳板/直连殿后
            return new ArrayList<>(Arrays.asList(
                    ROUTE_TUNNEL, ROUTE_TS, ROUTE_FUNNEL, ROUTE_TSNET, ROUTE_JUMP, ROUTE_LAN));
        }
        List<String> order = new ArrayList<>(seen);
        for (String id : defaultRouteOrder()) {
            if (!order.contains(id)) {
                order.add(id);
            }
        }
        return order;
    }

    /**
     * 按优先级产出启用且可用的连接候选。
     * lan / jump 的 origin 留空（消费方用自己的局域网端点与探测）；
     * jump 需先配好跳板机档案，否则视为不可用。
     *
     * <p>配对区块**逐条展开**：同一个区块里三条配对就是三个候选，各自带自己的地址、
     * 令牌与超时（不再「一个通道一个地址」）。
     */
    List<RouteCandidate> routeCandidates() {
        List<RouteCandidate> out = new ArrayList<>();
        boolean hasLan = false;
        for (String address : addresses) {
            if (!text(address).isEmpty()) {
                hasLan = true;
                break;
            }
        }
        // 配对区块整体被禁用（route_disabled）时整块跳过，剩余的是逐条 enabled
        LinkedHashSet<String> doneBlocks = new LinkedHashSet<>();
        for (String kind : routeOrderSorted()) {
            if (routeDisabled.contains(kind)) {
                continue;
            }
            if (ROUTE_LAN.equals(kind)) {
                if (hasLan) {
                    out.add(new RouteCandidate(kind, "", "", probeTimeoutFor(kind), ""));
                }
                continue;
            }
            if (ROUTE_JUMP.equals(kind)) {
                if (hasLan && !text(jumpServerId).isEmpty()) {
                    out.add(new RouteCandidate(kind, "", "", probeTimeoutFor(kind), ""));
                }
                continue;
            }
            String block = RemotePair.blockOfRoute(kind);
            if (block.isEmpty() || !doneBlocks.add(block)) {
                // ts 与 funnel 同属 ts 区块：整块只展开一次（否则同一批配对报两遍）
                continue;
            }
            for (RemotePair pair : pairsOfBlock(block, true)) {
                // 通道停用按 kind 粒度：ts 区块里「直连」与「Funnel」是优先级表里
                // 两行独立的通道（ts / funnel），停用哪一个只排掉哪一条 lane
                //（与 Rust route_candidates、PC 端 pcRouteCandidates 同口径）
                String pairKind = pair.routeKind();
                if (!pairKind.isEmpty() && routeDisabled.contains(pairKind)) {
                    continue;
                }
                out.add(new RouteCandidate(
                        pair.routeKind(),
                        normalizeRemoteOrigin(pair.addr),
                        text(pair.token),
                        probeTimeoutForPair(pair),
                        text(pair.id)));
            }
        }
        return out;
    }

    /** 优先级排序下「远程通道在前」：第一个启用的候选就是远程通道（connMode 的替代语义） */
    boolean routePriorityRemoteFirst() {
        List<RouteCandidate> candidates = routeCandidates();
        if (candidates.isEmpty()) {
            return false;
        }
        return candidates.get(0).isRemote();
    }

    ServerProfile() {
        this.id = UUID.randomUUID().toString();
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
    // 远程接入（公网隧道兜底）
    // ------------------------------------------------------------------

    /** 归一化后的远程地址（scheme://host）；没配返回空串 —— 就是生效配对的地址 */
    String remoteOrigin() {
        return effectiveRemoteAddr();
    }

    /**
     * 用粘贴内容加入一条配对（整条 {@code anyboot://pair?v=2&…} 配对串或纯 URL 都行）。
     *
     * <p>返回落库的配对；解析失败返回 {@code null} 且不改动现有配对。
     * 归档规则见 {@code docs/remote-pairs.md} §4.6：同 {@code (mid, kind, lane)} 更新那一条
     * （保留位置与原 id），否则追加到区块末尾。
     */
    RemotePair addPairFromPaste(String raw) {
        ParsedPair parsed = parsePair(raw);
        if (parsed == null || !parsed.ok()) {
            return null;
        }
        return upsertPair(parsed);
    }

    /**
     * 把解析结果写进 {@link #pairs}：先按 {@code (mid, kind, lane)} 找，没有就按 {@code (addr, kind)}，
     * 都没有则追加。更新时**保留原 id 与首次配对时间**（探测结果才对得上、用户排好的顺序不被打乱）。
     */
    RemotePair upsertPair(ParsedPair parsed) {
        if (parsed == null || !parsed.ok()) {
            return null;
        }
        String addr = normalizeRemoteOrigin(parsed.tunnel);
        if (addr.isEmpty()) {
            return null;
        }
        String kind = parsed.kind.isEmpty() ? defaultBlockFor(addr) : parsed.kind;
        String mid = text(parsed.mid);
        // 幂等键的 lane 槽（与 RemotePair.key() 同口径）：ts 区块按地址形态分
        // direct / funnel —— 同一台电脑这两条并存，少比 lane 会让扫 funnel 码顶掉 direct 码
        String incomingLane = RemotePair.laneFor(kind, addr);
        RemotePair hit = null;
        for (RemotePair pair : pairs) {
            if (pair == null || !kind.equals(text(pair.kind).toLowerCase(Locale.US))) {
                continue;
            }
            if (!mid.isEmpty()) {
                String pairLane = RemotePair.KIND_TS.equals(kind) ? pair.effectiveLane() : "";
                if (mid.equals(text(pair.mid)) && pairLane.equals(incomingLane)) {
                    hit = pair;
                    break;
                }
            } else if (addr.equals(normalizeRemoteOrigin(pair.addr))) {
                hit = pair;
                break;
            }
        }
        if (hit == null) {
            if (pairs.size() >= MAX_PAIRS) {
                return null;
            }
            hit = new RemotePair(kind, addr);
            hit.id = RemotePair.newId();
            hit.createdAt = System.currentTimeMillis();
            pairs.add(hit);
        }
        hit.kind = kind;
        hit.addr = addr;
        if (!text(parsed.token).isEmpty()) {
            hit.token = text(parsed.token);
        }
        if (!mid.isEmpty()) {
            hit.mid = mid;
        }
        if (!text(parsed.os).isEmpty()) {
            hit.os = normalizeMachineOs(parsed.os);
        }
        if (!text(parsed.host).isEmpty()) {
            hit.host = text(parsed.host);
        }
        if (!text(parsed.tunnelId).isEmpty()) {
            hit.tunnelId = text(parsed.tunnelId);
        }
        hit.lane = RemotePair.laneFor(kind, addr);
        hit.enabled = true;
        return hit;
    }

    /** 区块 id 归一（老写法只在这条路（手填地址）上容忍，v2 配对串里非法值一律拒收） */
    static String blockFromLegacyType(String raw) {
        String value = text(raw).toLowerCase(Locale.US);
        switch (value) {
            case "cloudflare":
            case "cf":
            case "tunnel":
                return RemotePair.KIND_CLOUDFLARE;
            case "ts":
            case "tailscale":
            case "funnel":
                return RemotePair.KIND_TS;
            case "tsnet":
            case "builtin":
            case "builtin_ts":
                return RemotePair.KIND_TSNET;
            default:
                return "";
        }
    }

    /**
     * 没有显式类型时的兜底判型：*.ts.net → ts 区块（Funnel lane）、100.x → ts 区块（直连）、
     * 其余 → cloudflare。**不猜 tsnet** —— 内建 tsnet 与系统 Funnel 的域名同形，猜错就会
     * 把两条路混成一条。
     */
    static String defaultBlockFor(String origin) {
        String value = normalizeRemoteOrigin(origin);
        if (isFunnelOrigin(value) || isTailscaleAddr(value)) {
            return RemotePair.KIND_TS;
        }
        return RemotePair.KIND_CLOUDFLARE;
    }

    /** 清掉远程接入（所有配对） */
    void clearRemote() {
        pairs.clear();
    }

    /** 有没有配远程接入（任何一条启用的配对都算） */
    boolean hasRemote() {
        return !flatPairs().isEmpty();
    }

    /**
     * 有**启用中**的远程候选：连接优先级里排上、且没被 routeDisabled 停用的远程通道。
     *
     * <p>连接链路（远程优先判定 / 远程兜底）必须用它 —— {@link #hasRemote()} 只回答
     * 「配没配过配对」，用户把远程区块全关了之后还照它走隧道，连接策略就成了摆设。
     */
    boolean hasEnabledRemote() {
        for (RouteCandidate candidate : routeCandidates()) {
            if (candidate.isRemote()) {
                return true;
            }
        }
        return false;
    }

    /** 通道 id → 中文短标签（电脑页标签条 / 心跳标注用；与连接优先级表同名） */
    static String routeLabel(String kind) {
        switch (text(kind)) {
            case ROUTE_LAN:
                return "直连";
            case ROUTE_JUMP:
                return "跳板机";
            case ROUTE_TUNNEL:
                return "隧道";
            case ROUTE_TS:
                return "TS 直连";
            case ROUTE_FUNNEL:
                return "TS Funnel";
            case ROUTE_TSNET:
                return "内置 Funnel";
            default:
                return "";
        }
    }

    /** 连接优先是否「远程优先」（用户在编辑里选的；空 / auto = 局域网优先） */
    boolean isRemoteFirst() {
        return "remote_first".equals(text(connMode));
    }

    /**
     * 远程接入的 WebView 地址：{@code origin/?ab_token=…}（token 百分号编码）。
     *
     * <p>令牌取**生效配对自己的**那条，不再挂档案级单值令牌 —— 一台机器上
     * Cloudflare 隧道与 tsnet 是两次配对、两个令牌，共用一个必然有一条连不上。
     */
    String remoteUrl(String transport) {
        String origin = remoteOrigin();
        if (origin.isEmpty()) {
            return "";
        }
        StringBuilder url = new StringBuilder(origin).append("/");
        String secret = effectiveRemoteToken();
        if (!secret.isEmpty()) {
            url.append("?ab_token=").append(Uri.encode(secret));
        }
        String mode = text(transport);
        if (!mode.isEmpty() && !"auto".equals(mode)) {
            url.append(secret.isEmpty() ? "?" : "&").append("transport=").append(mode);
        }
        return url.toString();
    }

    /** 指定 origin 的 WebView 地址（多候选逐个尝试时用；令牌按 origin 找对应配对） */
    String remoteUrlFor(String origin, String transport) {
        String value = normalizeRemoteOrigin(origin);
        if (value.isEmpty()) {
            return "";
        }
        StringBuilder url = new StringBuilder(value).append("/");
        String secret = "";
        for (RemotePair pair : flatPairs()) {
            if (value.equals(normalizeRemoteOrigin(pair.addr))) {
                secret = text(pair.token);
                break;
            }
        }
        if (secret.isEmpty()) {
            secret = effectiveRemoteToken();
        }
        if (!secret.isEmpty()) {
            url.append("?ab_token=").append(Uri.encode(secret));
        }
        String mode = text(transport);
        if (!mode.isEmpty() && !"auto".equals(mode)) {
            url.append(secret.isEmpty() ? "?" : "&").append("transport=").append(mode);
        }
        return url.toString();
    }

    /**
     * 远程地址归一：整条 URL / 裸 host 都吃，剥路径与 query，无 scheme 补 https；
     * 例外：Tailscale CGNAT 段（100.64.0.0/10）补 http://（本机服务是明文 HTTP，
     * 一律补 https 会永远连不上 —— 与 PC 端 split_remote_address 同一条审计修复）。
     */
    static String normalizeRemoteOrigin(String raw) {
        String value = text(raw);
        if (value.isEmpty()) {
            return "";
        }
        String lower = value.toLowerCase(Locale.US);
        String scheme;
        if (lower.startsWith("https://")) {
            scheme = "https://";
            value = value.substring(8);
        } else if (lower.startsWith("http://")) {
            scheme = "http://";
            value = value.substring(7);
        } else if (isTailscaleAddr(value)) {
            // 无 scheme 的 Tailscale 裸地址：补 http://（本机服务是明文 HTTP）
            scheme = "http://";
        } else {
            scheme = "https://";
        }
        int end = value.length();
        int slash = value.indexOf('/');
        int question = value.indexOf('?');
        if (slash >= 0) {
            end = Math.min(end, slash);
        }
        if (question >= 0) {
            end = Math.min(end, question);
        }
        String authority = value.substring(0, end).trim();
        if (authority.isEmpty()) {
            return "";
        }
        // Tailscale 地址：补 http://；没带端口补默认端口（与 PC 端 with_default_port 对齐）
        if (scheme.equals("http://") && isTailscaleAddr(authority) && !authority.contains(":")) {
            authority = authority + ":12300";
        }
        return scheme + authority;
    }

    /**
     * 是否 Tailscale 地址：100.64.0.0/10 裸 IP（可带端口）或 *.ts.net 域名。
     * 与 PC 端 is_tailscale_host / hub.html isTailscaleAddr 同语义，改要三处对齐。
     */
    static boolean isTailscaleAddr(String addr) {
        String value = text(addr).trim();
        if (value.isEmpty()) {
            return false;
        }
        // 剥 scheme 与路径，只留 host[:port]
        String lower = value.toLowerCase(Locale.US);
        if (lower.startsWith("https://")) {
            value = value.substring(8);
        } else if (lower.startsWith("http://")) {
            value = value.substring(7);
        }
        int cut = value.length();
        int slash = value.indexOf('/');
        if (slash >= 0) {
            cut = Math.min(cut, slash);
        }
        value = value.substring(0, cut).trim();
        if (value.endsWith(".ts.net")) {
            return true;
        }
        if (value.contains(":")) {
            String[] parts = value.split(":");
            if (parts.length != 2) {
                return false; // 多个冒号是 IPv6 字面量
            }
            value = parts[0];
        }
        String[] octets = value.split("\\.");
        if (octets.length < 2) {
            return false;
        }
        try {
            int a = Integer.parseInt(octets[0]);
            int b = Integer.parseInt(octets[1]);
            return a == 100 && b >= 64 && b <= 127;
        } catch (NumberFormatException error) {
            return false;
        }
    }

    /** 配对内容解析结果（成功或失败都用它传，失败时 {@link #error} 是给用户看的原因） */
    static final class ParsedPair {
        String mid = "";
        String os = "";
        String host = "";
        /** 区块 id：cloudflare / ts / tsnet */
        String kind = "";
        String tunnel = "";
        String token = "";
        String tunnelId = "";
        String error = "";

        boolean ok() {
            return error.isEmpty() && !tunnel.isEmpty();
        }
    }

    /**
     * 解析粘贴的配对内容：{@code anyboot://pair?v=2&mid=…&os=…&host=…&type=…&tunnel=…&token=…&tunnel_id=…}
     * 或直接一条 {@code https://…/?ab_token=…}。
     *
     * <p><b>配对串只认 {@code v=2}</b>（{@code docs/remote-pairs.md} §4.5）：老串没有
     * `mid` / `type`，硬解出来的条目既无法去重也判不出区块，结果是「配一个顶掉另一个」。
     * 所以老串一律拒收，并让用户回电脑上重新生成。
     *
     * <p>{@code type} 必须是区块 id（cloudflare / ts / tsnet）；非法值同样拒收 ——
     * 猜类型正是历史 bug 的源头（三套 {@code *.ts.net} 长得一模一样）。
     *
     * <p>裸 URL 走另一条路：没有声明类型，按地址形态判（100.x / *.ts.net → ts，其余 cloudflare）。
     */
    static ParsedPair parsePair(String raw) {
        String value = text(raw);
        ParsedPair out = new ParsedPair();
        if (value.isEmpty()) {
            out.error = "没认出配对码 —— 要 anyboot://pair?v=2&… 串，或 https://… / http://100.x.y.z:12300 地址";
            return out;
        }
        String lower = value.toLowerCase(Locale.US);
        if (lower.startsWith("anyboot://pair?")) {
            Uri uri;
            try {
                uri = Uri.parse(value);
            } catch (Exception error) {
                out.error = "配对码不是合法 URL";
                return out;
            }
            String version = text(uri.getQueryParameter("v"));
            if (version.isEmpty()) {
                out.error = "这是旧版配对码（缺 v=2），请在电脑上重新生成一个";
                return out;
            }
            if (!"2".equals(version)) {
                out.error = "配对码版本不支持（v=" + version + "），请在电脑上重新生成一个";
                return out;
            }
            String type = text(uri.getQueryParameter("type")).toLowerCase(Locale.US);
            if (!RemotePair.isBlockId(type)) {
                out.error = "配对码里的 type 非法（"
                        + (type.isEmpty() ? "缺失" : type)
                        + "），要 cloudflare / ts / tsnet";
                return out;
            }
            out.kind = type;
            out.tunnel = normalizeRemoteOrigin(uri.getQueryParameter("tunnel"));
            out.mid = text(uri.getQueryParameter("mid"));
            out.os = normalizeMachineOs(uri.getQueryParameter("os"));
            String host = text(uri.getQueryParameter("host"));
            out.host = host.isEmpty() ? text(uri.getQueryParameter("gateway")) : host;
            out.token = text(uri.getQueryParameter("token"));
            if (out.token.isEmpty()) {
                out.token = text(uri.getQueryParameter("ab_token"));
            }
            out.tunnelId = text(uri.getQueryParameter("tunnel_id"));
            if (out.tunnel.isEmpty()) {
                out.error = "配对码里没有可用地址（tunnel 为空）—— 让对方先开启通道再重新出码";
            }
            return out;
        }
        // 直接粘 URL / 裸 host：挖 ab_token / token 参数，类型按地址形态判
        String origin = normalizeRemoteOrigin(value);
        if (origin.isEmpty()) {
            out.error = "没认出地址";
            return out;
        }
        out.tunnel = origin;
        try {
            Uri uri = Uri.parse(value);
            String candidate = uri.getQueryParameter("ab_token");
            if (candidate == null || candidate.trim().isEmpty()) {
                candidate = uri.getQueryParameter("token");
            }
            out.token = text(candidate);
            out.kind = blockFromLegacyType(uri.getQueryParameter("type"));
        } catch (Exception ignored) {
            // 无 query 或不是合法 URL：就当裸 host
        }
        if (out.kind.isEmpty()) {
            out.kind = defaultBlockFor(origin);
        }
        return out;
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

    /**
     * 标签文本 = 显示名。
     *
     * <p>原来会拼上「分组名 · 」，分组连同折叠 UI 一起删掉了（{@code docs/remote-pairs.md}
     * §4.4 / §4.3）：一台机器同时有 cloudflare + ts + tsnet 是常态，用户已经在配对区块里
     * 看到「来自哪台机器、哪条路」，再加一层分组只是同义反复。
     */
    String label() {
        return displayName();
    }

    /** 排序键：按显示名，再按首选地址（没有分组前缀了） */
    String sortKey() {
        return displayName() + "\u0000" + host();
    }

    /** 设备色调：同一台机器恒定同色（取 machineId / id 的哈希定色相，替代原分组配色） */
    int deviceColor() {
        String seed = text(machineId);
        if (seed.isEmpty()) {
            seed = text(id);
        }
        if (seed.isEmpty()) {
            return 0xFFB9BFC9;
        }
        int hash = 0;
        for (int i = 0; i < seed.length(); i++) {
            hash = hash * 31 + seed.charAt(i);
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
     * <p>只看电脑端的机器标识 {@link #machineId}：分组名已经删掉，而「主机名相同」
     * 会误判（同名的两台 Windows 很常见）。任一边没有标识就当不是同一台 ——
     * 宁可多一条记录，也不要把两台机器合成一条。
     */
    boolean sameMachine(ServerProfile other) {
        if (other == null || other == this || id.equals(other.id)) {
            return false;
        }
        String mine = text(machineId);
        String theirs = text(other.machineId);
        return !mine.isEmpty() && mine.equalsIgnoreCase(theirs);
    }

    /**
     * 「连的东西」是否等价：地址、局域网令牌、通道顺序、配对条目（含各自令牌与启用状态）、
     * 区块策略、探测超时都算进去。
     *
     * <p>用来决定「档案被改了要不要重连」。以前比对的是 {@code remoteAddr} 这个单值镜像，
     * 现在没有镜像了 —— 同一个生效地址背后可能换了一条配对（令牌不同、超时不同），
     * 只比地址会漏掉「换了一条配对但地址恰好一样」的情况。
     */
    static boolean sameConnState(ServerProfile a, ServerProfile b) {
        if (a == b) {
            return true;
        }
        if (a == null || b == null) {
            return false;
        }
        if (!a.addresses.equals(b.addresses)) {
            return false;
        }
        if (!text(a.token).equals(text(b.token))) {
            return false;
        }
        if (!a.routeOrderSorted().equals(b.routeOrderSorted())) {
            return false;
        }
        if (!a.routeDisabled.equals(b.routeDisabled)) {
            return false;
        }
        if (a.probeTimeoutMs != b.probeTimeoutMs) {
            return false;
        }
        if (!a.blockProbeMode.equals(b.blockProbeMode) || !a.blockTimeoutsMs.equals(b.blockTimeoutsMs)) {
            return false;
        }
        List<RemotePair> mine = a.flatPairs();
        List<RemotePair> theirs = b.flatPairs();
        if (mine.size() != theirs.size()) {
            return false;
        }
        for (int i = 0; i < mine.size(); i++) {
            RemotePair x = mine.get(i);
            RemotePair y = theirs.get(i);
            if (!x.kind.equals(y.kind) || !x.addr.equals(y.addr) || !x.token.equals(y.token)) {
                return false;
            }
        }
        return true;
    }

    ServerProfile copy() {
        ServerProfile other = new ServerProfile();
        other.id = id;
        other.name = name;
        other.addresses.addAll(addresses);
        for (Map.Entry<String, List<String>> entry : macs.entrySet()) {
            other.macs.put(entry.getKey(), new ArrayList<>(entry.getValue()));
        }
        other.token = token;
        other.machineId = machineId;
        other.machineName = machineName;
        other.machineOs = machineOs;
        other.probeMode = probeMode;
        other.routeProbeMode = routeProbeMode;
        other.probeTimeoutMs = probeTimeoutMs;
        other.routeTimeoutsMs = new LinkedHashMap<>(routeTimeoutsMs == null
                ? new LinkedHashMap<>() : routeTimeoutsMs);
        other.jumpServerId = jumpServerId;
        // 配对必须深拷贝：编辑弹窗改副本、取消时原档案不受影响（浅拷贝会把条目一起改掉）
        for (RemotePair pair : pairs) {
            if (pair != null) {
                other.pairs.add(pair.copy());
            }
        }
        if (blockProbeMode != null) {
            other.blockProbeMode.putAll(blockProbeMode);
        }
        if (blockTimeoutsMs != null) {
            other.blockTimeoutsMs.putAll(blockTimeoutsMs);
        }
        other.connMode = connMode;
        other.routeOrder.addAll(routeOrder);
        other.routeDisabled.addAll(routeDisabled);
        other.routeProbeHidden.addAll(routeProbeHidden);
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
        object.put("machineOs", normalizeMachineOs(machineOs));
        object.put("probeMode", normalizeProbeMode(probeMode));
        object.put("routeProbeMode", normalizeRouteProbeMode(routeProbeMode));
        object.put("probeTimeoutMs", clampProbeTimeoutMs(probeTimeoutMs));
        // 每通道自己的超时：只写单独设过的（空 map 也让字段在，读的时候好判断）
        JSONObject timeouts = new JSONObject();
        for (Map.Entry<String, Integer> entry : normalizeRouteTimeouts(routeTimeoutsMs).entrySet()) {
            timeouts.put(entry.getKey(), entry.getValue());
        }
        object.put("routeTimeoutsMs", timeouts);
        object.put("jumpServerId", text(jumpServerId));
        // 配对条目：字段名与 PC 端 `RemotePair` 逐字一致（docs/remote-pairs.md §4.1）
        JSONArray pairArray = new JSONArray();
        for (RemotePair pair : pairs) {
            if (pair != null) {
                pairArray.put(pair.toJson());
            }
        }
        object.put("pairs", pairArray);
        JSONObject blockModes = new JSONObject();
        for (Map.Entry<String, String> entry : blockProbeMode.entrySet()) {
            String mode = normalizeProbeMode(entry.getValue());
            // 默认值不落盘：等号默认的字段写出去只会让配置文件越读越乱
            if (!PROBE_PARALLEL_FIRST.equals(mode) && RemotePair.isBlockId(entry.getKey())) {
                blockModes.put(entry.getKey(), mode);
            }
        }
        object.put("block_probe_mode", blockModes);
        JSONObject blockTimeouts = new JSONObject();
        for (Map.Entry<String, Integer> entry : blockTimeoutsMs.entrySet()) {
            if (entry.getValue() != null && entry.getValue() > 0 && RemotePair.isBlockId(entry.getKey())) {
                blockTimeouts.put(entry.getKey(), clampProbeTimeoutMs(entry.getValue()));
            }
        }
        object.put("block_timeouts_ms", blockTimeouts);
        object.put("connMode", isRemoteFirst() ? "remote_first" : "");
        // 五通道优先级：落盘用自愈后的有序列表（老档案在此顺手物化迁移结果）
        JSONArray routeOrderArray = new JSONArray();
        for (String kind : routeOrderSorted()) {
            routeOrderArray.put(kind);
        }
        object.put("routeOrder", routeOrderArray);
        JSONArray routeDisabledArray = new JSONArray();
        for (String kind : routeDisabled) {
            routeDisabledArray.put(kind);
        }
        object.put("routeDisabled", routeDisabledArray);
        // 「是否展示连通性」关掉的通道（与 routeDisabled 同为 camelCase 落盘；
        // canonical 备份同键，PC 端 backup 层负责与 route_probe_hidden 互转）
        JSONArray routeProbeHiddenArray = new JSONArray();
        for (String kind : routeProbeHidden) {
            routeProbeHiddenArray.put(kind);
        }
        object.put("routeProbeHidden", routeProbeHiddenArray);
        object.put("updatedAt", updatedAt);
        return object;
    }

    static ServerProfile fromJson(JSONObject object) {
        ServerProfile profile = new ServerProfile();
        String id = object.optString("id", "");
        if (!id.isEmpty()) {
            profile.id = id;
        }
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
        profile.machineOs = normalizeMachineOs(object.optString("machineOs", ""));
        // 只认三个值，其余（含缺失 / 拼错 / 旧 parallel）一律回落到并发（首个）
        profile.probeMode = normalizeProbeMode(object.optString("probeMode", ""));
        profile.routeProbeMode = normalizeRouteProbeMode(object.optString("routeProbeMode", ""));
        // 老档案没有 probeTimeoutMs → 默认 2 秒；越界在 clamp 里夹
        profile.probeTimeoutMs = clampProbeTimeoutMs(object.optInt("probeTimeoutMs", DEFAULT_PROBE_TIMEOUT_MS));
        // 每通道自己的超时：老档案没有这个字段 → 空表（全部继承上面的默认值）
        Map<String, Integer> perChannel = new LinkedHashMap<>();
        JSONObject timeouts = object.optJSONObject("routeTimeoutsMs");
        if (timeouts != null) {
            java.util.Iterator<String> keys = timeouts.keys();
            while (keys.hasNext()) {
                String key = keys.next();
                perChannel.put(key, timeouts.optInt(key, 0));
            }
        }
        profile.routeTimeoutsMs = normalizeRouteTimeouts(perChannel);
        profile.jumpServerId = object.optString("jumpServerId", "");
        // 配对条目：按 (mid,kind) 归一化去重，保留原位与原 id（与 PC 端 normalize_pairs 同语义）
        profile.pairs.clear();
        JSONArray pairArray = object.optJSONArray("pairs");
        if (pairArray != null) {
            for (int i = 0; i < pairArray.length(); i++) {
                RemotePair pair = RemotePair.fromJson(pairArray.optJSONObject(i));
                if (pair == null || text(pair.addr).isEmpty()) {
                    continue;
                }
                if (text(pair.kind).isEmpty()) {
                    pair.kind = defaultBlockFor(pair.addr);
                }
                if (!RemotePair.isBlockId(pair.kind)) {
                    continue;
                }
                if (text(pair.id).isEmpty()) {
                    pair.id = RemotePair.newId();
                }
                String key = pair.key();
                RemotePair existing = null;
                for (RemotePair seen : profile.pairs) {
                    if (seen.key().equals(key)) {
                        existing = seen;
                        break;
                    }
                }
                if (existing == null) {
                    if (profile.pairs.size() < MAX_PAIRS) {
                        profile.pairs.add(pair);
                    }
                } else {
                    // 同一条重复出现：地址/令牌以靠后的为准，id 与首次时间留在原位
                    existing.addr = pair.addr;
                    if (!text(pair.token).isEmpty()) {
                        existing.token = pair.token;
                    }
                    if (!text(pair.tunnelId).isEmpty()) {
                        existing.tunnelId = pair.tunnelId;
                    }
                }
            }
        }
        JSONObject blockModes = object.optJSONObject("block_probe_mode");
        if (blockModes != null) {
            java.util.Iterator<String> keys = blockModes.keys();
            while (keys.hasNext()) {
                String block = keys.next();
                if (RemotePair.isBlockId(block)) {
                    profile.blockProbeMode.put(block.toLowerCase(Locale.US),
                            normalizeProbeMode(blockModes.optString(block, "")));
                }
            }
        }
        JSONObject blockTimeouts = object.optJSONObject("block_timeouts_ms");
        if (blockTimeouts != null) {
            java.util.Iterator<String> keys = blockTimeouts.keys();
            while (keys.hasNext()) {
                String block = keys.next();
                if (RemotePair.isBlockId(block)) {
                    profile.blockTimeoutsMs.put(block.toLowerCase(Locale.US),
                            clampProbeTimeoutMs(blockTimeouts.optInt(block, 0)));
                }
            }
        }
        profile.connMode = "remote_first".equals(object.optString("connMode", "")) ? "remote_first" : "";
        profile.updatedAt = object.optLong("updatedAt", 0);
        // 六通道优先级：清洗非法 id / 去重；全空按 connMode 迁移（routeOrderSorted 的自愈语义）
        JSONArray routeOrderArray = object.optJSONArray("routeOrder");
        if (routeOrderArray != null) {
            for (int i = 0; i < routeOrderArray.length(); i++) {
                String kind = routeOrderArray.optString(i, "").trim();
                if (isRouteKind(kind) && !profile.routeOrder.contains(kind)) {
                    profile.routeOrder.add(kind);
                }
            }
        }
        JSONArray routeDisabledArray = object.optJSONArray("routeDisabled");
        if (routeDisabledArray != null) {
            for (int i = 0; i < routeDisabledArray.length(); i++) {
                String kind = routeDisabledArray.optString(i, "").trim();
                // 认全部六个通道 id（与 PC 端 2026-10-28 修复对齐）：以前只留 lan / jump，
                // 用户关掉 Cloudflare/Tailscale 通道保存后重读档案就弹回，探测照走。
                // 配对条目自己的开关（RemotePair.enabled）与这里互不覆盖。
                if (isRouteKind(kind) && !profile.routeDisabled.contains(kind)) {
                    profile.routeDisabled.add(kind);
                }
            }
        }
        // 展示连通性被关掉的通道：同样清洗（只认合法 id + 去重），与 PC 端
        // route_probe_hidden 的 normalize 同语义
        JSONArray routeProbeHiddenArray = object.optJSONArray("routeProbeHidden");
        if (routeProbeHiddenArray == null) {
            routeProbeHiddenArray = object.optJSONArray("route_probe_hidden");
        }
        if (routeProbeHiddenArray != null) {
            for (int i = 0; i < routeProbeHiddenArray.length(); i++) {
                String kind = routeProbeHiddenArray.optString(i, "").trim();
                if (isRouteKind(kind) && !profile.routeProbeHidden.contains(kind)) {
                    profile.routeProbeHidden.add(kind);
                }
            }
        }
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

    /**
     * 新增电脑时预填的「局域网电源策略」（用户 2026-10-31）：四个动作各一条，
     * 端口按 sol 的分配 —— 唤醒 9 / 睡眠 10 / 关机 11 / 重启 12；<b>默认全关</b> ——
     * 只替用户把骨架搭好，开哪条自己定；发包 / 重试 10、发送模式「优先广播包」。
     * 停用的策略不参与发送（{@link #routesFor} 过滤 enabled），新电脑电源行为与从前一致。
     * 与电脑端 {@code default_power_routes()} / hub.html {@code defaultPowerRoutes()} 三端一致。
     */
    static List<PowerRoute> defaultPowerRoutes() {
        String[] actions = {POWER_WAKE, POWER_SLEEP, POWER_SHUTDOWN, POWER_REBOOT};
        int[] ports = {9, 10, 11, 12};
        List<PowerRoute> result = new ArrayList<>();
        for (int i = 0; i < actions.length; i++) {
            PowerRoute route = new PowerRoute(actions[i], "", ports[i]);
            route.enabled = false;
            route.burst = 10;
            route.retry = 10;
            route.mode = "auto";
            route.httpFallback = true;
            result.add(route);
        }
        return result;
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

    // ------------------------------------------------------------------
    // 访客主题（查看方偏好，与网页端 abTheme 参数同一约定）
    // ------------------------------------------------------------------
    //
    // WebView 加载的是**那台电脑**自己的页面：跨来源，localStorage 各存各的桶，
    // 版式主题偏好互不相通（看一台就得重选一次，还让人以为「改了他的主题」）。
    // App 把「我选的主题」经 URL（abTheme=<字母>）带过去：页面照它渲染，但
    // 不读也不写它自己桶里的主题 —— 「不改掉他的主题，用我的主题看」。

    /** 版式主题字母（与网页端 THEME_LAYOUT_MODES 的键一致；首项 "" = 不带参数，跟随电脑自己保存的主题） */
    static final String[] BOOT_THEMES = {"", "K", "A", "B", "C", "G", "I", "J"};

    /** 查看方的引导页主题偏好（anyboot_ui/bootTheme；空 = 不带参数） */
    static String guestBootTheme(android.content.Context context) {
        try {
            return context.getSharedPreferences("anyboot_ui", android.content.Context.MODE_PRIVATE)
                .getString("bootTheme", "");
        } catch (Exception ignored) {
            return "";
        }
    }

    static void setGuestBootTheme(android.content.Context context, String theme) {
        String value = theme == null ? "" : theme.trim();
        boolean known = false;
        for (String candidate : BOOT_THEMES) {
            if (candidate.equals(value)) {
                known = true;
                break;
            }
        }
        if (!known) {
            value = "";
        }
        context.getSharedPreferences("anyboot_ui", android.content.Context.MODE_PRIVATE)
            .edit().putString("bootTheme", value).apply();
    }

    /** 给访问 URL 追加访客主题参数（url 空原样返回；这些 URL 恒带 ?token=…，直接拼 & 安全）。
     *  abUi=light|dark 亮暗主题：App 当前生效的亮暗（含跟随系统解析后）恒带过去，
     *  页面照它渲染，不读也不写那台电脑自己的亮暗设置；abTheme 同前（选过才带）。 */
    static String withGuestTheme(android.content.Context context, String url) {
        if (url == null || url.isEmpty()) {
            return url;
        }
        StringBuilder out = new StringBuilder(url);
        out.append("&abUi=").append(Ui.dark(context) ? "dark" : "light");
        String theme = guestBootTheme(context);
        if (!theme.isEmpty()) {
            out.append("&abTheme=").append(theme);
        }
        return out.toString();
    }

    /** 去首尾空白（所有可空字符串字段统一走这里，免得满地判 null） */
    static String text(String raw) {
        return raw == null ? "" : raw.trim();
    }
}
