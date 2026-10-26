package com.anyboot.remote;

import org.json.JSONObject;

import java.util.Locale;
import java.util.UUID;

/**
 * 一条远程配对 = 「某套 AnyBoot 配置的某一种接入方式」。
 *
 * <p>取代原来的四个单值槽（{@code remoteTunnelAddr} / {@code remoteTsAddr} /
 * {@code remoteFunnelAddr} / {@code remoteTsnetAddr}）+ 单值 {@code remoteToken}：
 * 一台机器可能同时有 Cloudflare 隧道、Tailscale 直连、系统 Funnel、内建 tsnet 四条路，
 * 而且同一个区块里还可能有**多条**（换过域名、新旧两条隧道并存）。四槽模型里同类型的
 * 第二条无处可放，配一个就顶掉另一个。字段名与语义与 PC 端 {@code RemotePair} 逐字一致
 * （{@code docs/remote-pairs.md} §4.1）。
 *
 * <p><b>{@link #kind} 是区块 id</b>（cloudflare / ts / tsnet），不是通道 id：
 * 通道 id（tunnel / ts / funnel / tsnet）在 {@link #routeKind()} 里算出来 ——
 * ts 区块内部再按 {@link #lane} 分成「100.x 直连」与「*.ts.net Funnel」两条通道。
 */
final class RemotePair {

    static final String KIND_CLOUDFLARE = "cloudflare";
    static final String KIND_TS = "ts";
    static final String KIND_TSNET = "tsnet";

    /** 三个区块 id（存储层；顺序即默认区块序） */
    static final String[] BLOCK_IDS = { KIND_CLOUDFLARE, KIND_TS, KIND_TSNET };

    static final String LANE_DIRECT = "direct";
    static final String LANE_FUNNEL = "funnel";

    /**
     * 幂等键的 lane 槽（唯一计算口径）：ts 区块按地址形态分 direct / funnel，其余区块恒空。
     *
     * <p>upsertPair 的命中比较、编辑表单的粘贴分拣、保存回写三处都用它 —— lane 是地址的
     * 派生值，不单独存储判断，避免四处口径漂移（漂移一次就是一次「互顶」）。
     */
    static String laneFor(String kind, String addr) {
        if (!KIND_TS.equals(text(kind).toLowerCase(Locale.US))) {
            return "";
        }
        return ServerProfile.isFunnelOrigin(addr) ? LANE_FUNNEL : LANE_DIRECT;
    }

    String id = "";
    /** 区块 id：cloudflare / ts / tsnet */
    String kind = "";
    /** 只有 ts 区块有意义：direct / funnel（写空则按地址形态推导） */
    String lane = "";
    /** 归一后的 origin（{@code https://x.trycloudflare.com} / {@code http://100.64.0.7:12300}） */
    String addr = "";
    /** 这条自己的配对令牌（签发端按客户端发的，与档案的局域网 token 是两套） */
    String token = "";
    /** 0 = 继承区块默认 → 全局默认 */
    int probeTimeoutMs = 0;
    /** 停用只是不探测，条目本身留着 */
    boolean enabled = true;
    /** 对方系统自报的 machine_id —— 幂等键的一半（另一半是 kind） */
    String mid = "";
    /** windows / macos / linux（列表徽标 + 来源显示） */
    String os = "";
    /** 对方主机名（纯显示） */
    String host = "";
    /** named tunnel 的 id（识别同一隧道被多套系统共用） */
    String tunnelId = "";
    String note = "";
    long createdAt = 0L;

    RemotePair() {
    }

    RemotePair(String kind, String addr) {
        this.kind = text(kind);
        this.addr = text(addr);
    }

    /** 区块 id 是否合法 */
    static boolean isBlockId(String raw) {
        String value = text(raw).toLowerCase(Locale.US);
        for (String block : BLOCK_IDS) {
            if (block.equals(value)) {
                return true;
            }
        }
        return false;
    }

    /** 通道 id → 区块 id（lan / jump 不属于任何区块，返回空串） */
    static String blockOfRoute(String routeKind) {
        String value = text(routeKind);
        switch (value) {
            case ServerProfile.ROUTE_TUNNEL:
                return KIND_CLOUDFLARE;
            case ServerProfile.ROUTE_TS:
            case ServerProfile.ROUTE_FUNNEL:
                return KIND_TS;
            case ServerProfile.ROUTE_TSNET:
                return KIND_TSNET;
            default:
                return "";
        }
    }

    static String blockLabel(String block) {
        switch (text(block)) {
            case KIND_CLOUDFLARE:
                return "Cloudflare";
            case KIND_TS:
                return "Tailscale";
            case KIND_TSNET:
                return "Tailscale（内建）";
            default:
                return "";
        }
    }

    /**
     * 上报/展示用的通道 id（{@code route_order} / ProbeOutcome.kind 那一套）。
     * 空 = 这条配对不可用（kind 非法）。
     */
    String routeKind() {
        switch (text(kind).toLowerCase(Locale.US)) {
            case KIND_CLOUDFLARE:
                return ServerProfile.ROUTE_TUNNEL;
            case KIND_TSNET:
                return ServerProfile.ROUTE_TSNET;
            case KIND_TS:
                return LANE_FUNNEL.equals(effectiveLane())
                        ? ServerProfile.ROUTE_FUNNEL : ServerProfile.ROUTE_TS;
            default:
                return "";
        }
    }

    /** 这条配对的 lane（写空了的按地址形态补：100.64/10 → direct，*.ts.net → funnel） */
    String effectiveLane() {
        String value = text(lane).toLowerCase(Locale.US);
        if (LANE_DIRECT.equals(value)) {
            return LANE_DIRECT;
        }
        if (LANE_FUNNEL.equals(value)) {
            return LANE_FUNNEL;
        }
        return ServerProfile.isFunnelOrigin(addr) ? LANE_FUNNEL : LANE_DIRECT;
    }

    /**
     * 幂等键：有 mid 用 {@code mid|kind|lane|mid}，没有 mid 用 {@code addr|kind|addr}。
     *
     * <p>不能只用 mid —— 同一套系统可以同时有 cloudflare + ts + tsnet；也不能只用
     * {@code (mid, kind)} —— ts 区块合并了「直连」与「系统 Funnel」，同一台电脑这两条
     * 是<b>并存</b>的配对（lane 区分），少比 lane 会让扫 funnel 码顶掉 direct 码，
     * 互顶从区块间挪进了区块内。lane 只对 ts 区块有区分度（其余恒空）；
     * {@link #effectiveLane()} 让 lane 写空的老数据与新数据同键。
     * 空 mid 时按地址去重：地址形态本身就区分 direct / funnel。
     */
    String key() {
        String machine = text(mid);
        if (!machine.isEmpty()) {
            String laneSlot = KIND_TS.equals(text(kind).toLowerCase(Locale.US)) ? effectiveLane() : "";
            return "mid|" + text(kind) + "|" + laneSlot + "|" + machine;
        }
        return "addr|" + text(kind) + "|" + text(addr);
    }

    /** 拷贝（编辑弹窗里改副本，取消时原档案不受影响） */
    RemotePair copy() {
        RemotePair other = new RemotePair();
        other.id = id;
        other.kind = kind;
        other.lane = lane;
        other.addr = addr;
        other.token = token;
        other.probeTimeoutMs = probeTimeoutMs;
        other.enabled = enabled;
        other.mid = mid;
        other.os = os;
        other.host = host;
        other.tunnelId = tunnelId;
        other.note = note;
        other.createdAt = createdAt;
        return other;
    }

    /** 行标题：{@code <host> · <kind>}；host 空就只写区块名 */
    String title() {
        String machine = text(host);
        String label = text(kind);
        if (machine.isEmpty()) {
            return label;
        }
        return machine + " · " + label;
    }

    /** 来源列：{@code host · os}；都空时退回 mid 短号（再空就空串） */
    String sourceLabel() {
        StringBuilder out = new StringBuilder();
        if (!text(host).isEmpty()) {
            out.append(text(host));
        }
        if (!text(os).isEmpty()) {
            if (out.length() > 0) {
                out.append(" · ");
            }
            out.append(text(os));
        }
        if (out.length() == 0 && !text(mid).isEmpty()) {
            String value = text(mid);
            out.append(value.length() > 8 ? value.substring(0, 8) : value);
        }
        return out.toString();
    }

    JSONObject toJson() {
        JSONObject object = new JSONObject();
        try {
            object.put("id", text(id));
            object.put("kind", text(kind));
            object.put("lane", text(lane));
            object.put("addr", ServerProfile.normalizeRemoteOrigin(addr));
            object.put("token", text(token));
            object.put("probe_timeout_ms", probeTimeoutMs <= 0 ? 0 : probeTimeoutMs);
            object.put("enabled", enabled);
            object.put("mid", text(mid));
            object.put("os", ServerProfile.normalizeMachineOs(os));
            object.put("host", text(host));
            object.put("tunnel_id", text(tunnelId));
            object.put("note", text(note));
            object.put("created_at", createdAt);
        } catch (Exception ignored) {
            // JSONObject.put(String, *) 只在 key 为 null 时抛，这里不可能
        }
        return object;
    }

    static RemotePair fromJson(JSONObject item) {
        if (item == null) {
            return null;
        }
        RemotePair pair = new RemotePair();
        pair.id = text(item.optString("id", ""));
        pair.kind = text(item.optString("kind", "")).toLowerCase(Locale.US);
        pair.lane = text(item.optString("lane", "")).toLowerCase(Locale.US);
        pair.addr = ServerProfile.normalizeRemoteOrigin(item.optString("addr", ""));
        pair.token = text(item.optString("token", ""));
        pair.probeTimeoutMs = item.optInt("probe_timeout_ms", 0);
        pair.enabled = item.optBoolean("enabled", true);
        pair.mid = text(item.optString("mid", ""));
        pair.os = ServerProfile.normalizeMachineOs(item.optString("os", ""));
        pair.host = text(item.optString("host", ""));
        pair.tunnelId = text(item.optString("tunnel_id", ""));
        pair.note = text(item.optString("note", ""));
        pair.createdAt = item.optLong("created_at", 0L);
        return pair;
    }

    static String text(String raw) {
        return raw == null ? "" : raw.trim();
    }

    static String newId() {
        return UUID.randomUUID().toString();
    }
}
