package com.anyboot.remote;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.ThreadFactory;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * **按「连接策略」探测**：两层策略，与 PC 端 `probe_candidates_blocking` 同一套语义。
 *
 * <ul>
 *   <li><b>外层</b> {@link ServerProfile#routeProbeMode} —— 连接优先级的探测方式，管
 *       <b>六个通道之间</b>怎么试（并发首个 / 并发全部 / 串行），对每一个通道都生效；</li>
 *   <li><b>内层</b> {@link ServerProfile#probeMode} —— 多网卡探测，管
 *       <b>同一个通道里的多个地址</b>（实际只有局域网会配多个）。</li>
 * </ul>
 * 两层是独立开关，组合语义清楚：「通道之间并发、每个通道的内部多网卡按顺序试」也合法。
 * 每个通道的探测超时再各自取 {@link ServerProfile#probeTimeoutFor(String)}。
 *
 * <p>为什么要有这一层：以前「电脑」页与「电脑列表」各写一套探活 —— 列表只看局域网地址，
 * 主页面才认远程通道，于是同一台电脑在两个页面的在线状态能不一致；探测方式也只作用于
 * 多网卡。现在两页共用这里的结果。
 *
 * <p>线程池是**探测专用**的（{@link #Holder}）：心跳每 10 秒一轮，弱网/离线时每个地址
 * 都要等满超时，如果和隧道、JSON 请求抢同一个池，界面会跟着卡。
 */
final class ProbeEngine {

    /** 一次探测的结果行（一个候选一条） */
    static final class Hit {
        final String kind;
        final String origin;
        final boolean ok;
        final long ms;
        final String error;
        final String machineId;
        final String machineName;
        final String os;

        Hit(String kind, String origin, boolean ok, long ms, String error,
            String machineId, String machineName, String os) {
            this.kind = kind;
            this.origin = origin;
            this.ok = ok;
            this.ms = ms;
            this.error = error;
            this.machineId = machineId;
            this.machineName = machineName;
            this.os = os;
        }
    }

    /** 一台电脑的整体结论：赢家 + 逐行结果 */
    static final class Result {
        boolean ok;
        String kind = "";
        String origin = "";
        long ms;
        String machineId = "";
        String machineName = "";
        String os = "";
        String error = "";
        List<Hit> hits = Collections.emptyList();

        /** 是否走了远程通道（隧道 / Tailscale） */
        boolean remote() {
            return isRemote(kind);
        }
    }

    /** 跳板机端点解析：探测线程里拿不到 Activity，交给调用方注入 */
    interface JumpResolver {
        /** 返回本机可访问的 host:port；解析不了返回 null */
        String resolve() throws Exception;
    }

    /**
     * 「本地」通道：局域网与跳板机。心跳先只探这一批 —— 本地通了就不用惊动公网
     * （省电，也避免每 10 秒往隧道打一次请求）；本地全挂才按策略兜底探远程。
     */
    static final List<String> LOCAL_KINDS = Collections.unmodifiableList(
            java.util.Arrays.asList(ServerProfile.ROUTE_LAN, ServerProfile.ROUTE_JUMP));

    /** 一个通道的候选组：同一个 kind 的多个 origin 归一组（只有局域网会有多个） */
    private static final class Group {
        final String kind;
        final List<String> origins = new ArrayList<>();

        Group(String kind) {
            this.kind = kind;
        }
    }

    /** 探测专用线程池（懒加载，守护线程，不阻止进程退出） */
    private static final class Holder {
        static final ExecutorService EXECUTOR = Executors.newFixedThreadPool(
                poolSize(), new ThreadFactory() {
                    private final AtomicInteger seq = new AtomicInteger();

                    @Override
                    public Thread newThread(Runnable runnable) {
                        Thread thread = new Thread(runnable, "anyboot-probe-" + seq.incrementAndGet());
                        thread.setDaemon(true);
                        return thread;
                    }
                });
    }

    private static int poolSize() {
        int cores = Runtime.getRuntime().availableProcessors();
        return Math.max(2, Math.min(8, cores));
    }

    private ProbeEngine() {
    }

    /**
     * 按档案的连接策略探一台电脑（全部通道）。
     *
     * @param jump 跳板机解析器（档案里没配跳板机可传 null）
     */
    static Result probe(ServerProfile profile, JumpResolver jump) {
        return probe(profile, jump, null);
    }

    /**
     * 只探指定通道（心跳分两阶段：先 {@link #LOCAL_KINDS}，本地全挂再传 null 探全部）。
     *
     * @param onlyKinds null = 全部通道；非 null = 只探这些 kind
     */
    static Result probe(ServerProfile profile, JumpResolver jump, List<String> onlyKinds) {
        Result result = new Result();
        if (profile == null) {
            result.error = "没有档案";
            return result;
        }
        List<String[]> candidates = expand(profile, onlyKinds);
        if (candidates.isEmpty()) {
            // 全通道路径要报错给用户看；限定通道的阶段性探测「没有候选」是正常情况
            result.error = onlyKinds == null ? "没有可用连接通道（地址为空，或通道都被停用）" : "";
            return result;
        }
        // 每个通道用自己的超时（通道没单独设 → 档案里的默认值）：局域网快、Funnel/tsnet 慢
        Map<String, Integer> timeouts = new HashMap<>();
        for (String[] candidate : candidates) {
            timeouts.put(candidate[0], profile.probeTimeoutFor(candidate[0]));
        }
        List<Group> groups = group(candidates);
        String routeMode = ServerProfile.normalizeProbeMode(profile.routeProbeMode);
        String addressMode = ServerProfile.normalizeProbeMode(profile.probeMode);
        return ServerProfile.PROBE_SERIAL.equals(routeMode)
                ? probeChannelsSerial(groups, addressMode, timeouts, jump)
                : probeChannelsParallel(groups, addressMode, timeouts, jump,
                        ServerProfile.PROBE_PARALLEL_ALL.equals(routeMode));
    }

    /**
     * routeCandidates 里 lan 只占一条（请求路径用），探测要**每个局域网地址各一条**，
     * 否则多网卡的机器永远只探第一块网卡。
     */
    private static List<String[]> expand(ServerProfile profile, List<String> onlyKinds) {
        List<String[]> out = new ArrayList<>();
        for (String[] candidate : profile.routeCandidates()) {
            String kind = candidate[0];
            if (onlyKinds != null && !onlyKinds.contains(kind)) {
                continue;
            }
            if (ServerProfile.ROUTE_LAN.equals(kind)) {
                for (String address : profile.addresses) {
                    String trimmed = ServerProfile.text(address);
                    if (!trimmed.isEmpty()) {
                        out.add(new String[]{kind, trimmed});
                    }
                }
            } else {
                out.add(candidate);
            }
        }
        return out;
    }

    /** 按 kind 分组，**保持候选出现顺序**（组顺序 = 用户在优先级表里排的顺序） */
    private static List<Group> group(List<String[]> candidates) {
        List<Group> groups = new ArrayList<>();
        for (String[] candidate : candidates) {
            Group hit = null;
            for (Group group : groups) {
                if (group.kind.equals(candidate[0])) {
                    hit = group;
                    break;
                }
            }
            if (hit == null) {
                hit = new Group(candidate[0]);
                groups.add(hit);
            }
            if (!hit.origins.contains(candidate[1])) {
                hit.origins.add(candidate[1]);
            }
        }
        return groups;
    }

    /** 这个通道该等多久（毫秒） */
    private static int timeoutOf(Map<String, Integer> timeouts, String kind) {
        Integer value = timeouts.get(kind);
        return value == null || value <= 0
                ? ServerProfile.DEFAULT_PROBE_TIMEOUT_MS
                : ServerProfile.clampProbeTimeoutMs(value);
    }

    /** 外层串行：一个通道一个通道地试，某个通道命中就不再往下走 */
    private static Result probeChannelsSerial(List<Group> groups, String addressMode,
                                              Map<String, Integer> timeouts, JumpResolver jump) {
        List<Hit> hits = new ArrayList<>();
        for (Group group : groups) {
            List<Hit> groupHits = probeGroup(group, addressMode, timeouts, jump);
            boolean ok = anyOk(groupHits);
            hits.addAll(groupHits);
            if (ok) {
                break;
            }
        }
        // 没轮到的通道如实标「没探」，逐行结果才能和优先级表对得上
        fillMissing(hits, groups, "前面已有通道命中，按串行策略未探测");
        return finish(hits);
    }

    /**
     * 外层并发：一个通道一个任务，组内再按内层（多网卡）策略处理它的多个地址。
     * `all=true` 等全部通道返回取最快成功者；`all=false` 第一个成功的通道立刻返回。
     */
    private static Result probeChannelsParallel(List<Group> groups, String addressMode,
                                                Map<String, Integer> timeouts,
                                                JumpResolver jump, boolean all) {
        final int expected = groups.size();
        final List<List<Hit>> ordered = new ArrayList<>(Collections.nCopies(expected, null));
        // 谁先回来谁先进队列 —— 必须按「完成先后」取，不能按提交顺序等
        final BlockingQueue<int[]> arrived = new LinkedBlockingQueue<>();
        int slowest = 0;
        for (int index = 0; index < expected; index++) {
            final Group group = groups.get(index);
            final int slot = index;
            final String inner = addressMode;
            final Map<String, Integer> map = timeouts;
            final JumpResolver resolver = jump;
            int per = timeoutOf(timeouts, group.kind);
            // 内层串行时这个通道最坏要等「地址数 × 超时」
            int worst = ServerProfile.PROBE_SERIAL.equals(inner)
                    ? per * Math.max(1, group.origins.size()) : per;
            slowest = Math.max(slowest, worst);
            Holder.EXECUTOR.execute(new Runnable() {
                @Override
                public void run() {
                    List<Hit> groupHits = probeGroup(group, inner, map, resolver);
                    // 按槽位摆回去（展示顺序 = 优先级顺序），再报告「通道完成」
                    synchronized (ordered) {
                        ordered.set(slot, groupHits);
                    }
                    arrived.offer(new int[]{slot});
                }
            });
        }
        long deadline = System.currentTimeMillis() + slowest + 500L;
        for (int count = 0; count < expected; count++) {
            long remaining = deadline - System.currentTimeMillis();
            int[] signal;
            try {
                signal = remaining > 0
                        ? arrived.poll(remaining, TimeUnit.MILLISECONDS)
                        : arrived.poll();
            } catch (InterruptedException interrupted) {
                Thread.currentThread().interrupt();
                break;
            }
            if (signal == null) {
                break;   // 到点了：还没回来的都按超时记
            }
            List<Hit> groupHits = ordered.get(signal[0]);
            if (groupHits == null) {
                continue;
            }
            // 并发首个：第一个成功的通道立刻收工，其余在途结果不再等
            if (!all && firstOk(groupHits) != null) {
                break;
            }
        }
        List<Hit> hits = new ArrayList<>();
        for (int index = 0; index < expected; index++) {
            List<Hit> groupHits = ordered.get(index);
            if (groupHits != null) {
                hits.addAll(groupHits);
            }
        }
        // 没回来的通道 / 地址如实标出来（带上它自己的超时值）
        for (Group group : groups) {
            int per = timeoutOf(timeouts, group.kind);
            for (String origin : group.origins) {
                if (contains(hits, group.kind, origin)) {
                    continue;
                }
                hits.add(new Hit(group.kind, origin, false, 0,
                        "探测超时（>" + per + "ms）", "", "", ""));
            }
        }
        return finish(hits);
    }

    /**
     * 探一个通道：组内多个地址按**多网卡探测**方式处理，返回按 origins 顺序排列的命中。
     * 未探测的地址也补一行，这样逐行结果永远和优先级表对得上。
     */
    private static List<Hit> probeGroup(Group group, String addressMode,
                                        Map<String, Integer> timeouts, JumpResolver jump) {
        int timeout = timeoutOf(timeouts, group.kind);
        List<Hit> hits = new ArrayList<>();
        if (group.origins.size() == 1) {
            hits.add(probeOne(group.kind, group.origins.get(0), timeout, jump));
            return hits;
        }
        if (ServerProfile.PROBE_SERIAL.equals(addressMode)) {
            // 内层串行：按用户排的地址顺序逐个试，第一个通就停
            for (String origin : group.origins) {
                Hit hit = probeOne(group.kind, origin, timeout, jump);
                hits.add(hit);
                if (hit.ok) {
                    break;
                }
            }
            for (int index = hits.size(); index < group.origins.size(); index++) {
                hits.add(new Hit(group.kind, group.origins.get(index), false, 0,
                        "同通道前面已命中，未探测", "", "", ""));
            }
            return hits;
        }
        // 内层并发：全部同时发出，按完成先后取（首个成功即收工 / 等全部）
        final int expected = group.origins.size();
        final Hit[] ordered = new Hit[expected];
        final BlockingQueue<Hit> arrived = new LinkedBlockingQueue<>();
        final boolean all = ServerProfile.PROBE_PARALLEL_ALL.equals(addressMode);
        List<Future<?>> futures = new ArrayList<>();
        for (int index = 0; index < expected; index++) {
            final String origin = group.origins.get(index);
            final String kind = group.kind;
            final int per = timeout;
            final int slot = index;
            final JumpResolver resolver = jump;
            futures.add(Holder.EXECUTOR.submit(new Runnable() {
                @Override
                public void run() {
                    Hit hit = probeOne(kind, origin, per, resolver);
                    ordered[slot] = hit;
                    arrived.offer(hit);
                }
            }));
        }
        long deadline = System.currentTimeMillis() + timeout + 500L;
        for (int count = 0; count < expected; count++) {
            long remaining = deadline - System.currentTimeMillis();
            Hit hit;
            try {
                hit = remaining > 0
                        ? arrived.poll(remaining, TimeUnit.MILLISECONDS)
                        : arrived.poll();
            } catch (InterruptedException interrupted) {
                Thread.currentThread().interrupt();
                break;
            }
            if (hit == null) {
                break;
            }
            if (hit.ok && !all) {
                for (Future<?> future : futures) {
                    future.cancel(true);
                }
                break;
            }
        }
        for (int index = 0; index < expected; index++) {
            Hit hit = ordered[index];
            hits.add(hit != null ? hit : new Hit(group.kind, group.origins.get(index), false, 0,
                    "同通道已命中，未再等待", "", "", ""));
        }
        return hits;
    }

    /** 探一个候选：局域网 → http://host:port/health；远程 → https origin + /health */
    private static Hit probeOne(String kind, String origin, int timeout, JumpResolver jump) {
        boolean remote = isRemote(kind);
        String target = origin;
        if (ServerProfile.ROUTE_JUMP.equals(kind)) {
            if (jump == null) {
                return new Hit(kind, "", false, 0, "没配跳板机解析器", "", "", "");
            }
            try {
                target = jump.resolve();
            } catch (Exception exception) {
                return new Hit(kind, "", false, 0, "跳板机不可用：" + exception.getMessage(), "", "", "");
            }
            if (target == null || target.trim().isEmpty()) {
                return new Hit(kind, "", false, 0, "跳板机隧道没建起来", "", "", "");
            }
            remote = false;
        }
        HealthProbe.Result probe = HealthProbe.probe(target, remote, timeout);
        if (probe == null) {
            return new Hit(kind, target, false, 0,
                    "连不上（超时或对方没起 HTTP 服务）", "", "", "");
        }
        return new Hit(kind, target, true, probe.ms,
                "", probe.machineId, probe.machineName, probe.os);
    }

    private static boolean isRemote(String kind) {
        return "tunnel".equals(kind) || "ts".equals(kind)
                || "funnel".equals(kind) || "tsnet".equals(kind);
    }

    private static boolean anyOk(List<Hit> hits) {
        for (Hit hit : hits) {
            if (hit.ok) {
                return true;
            }
        }
        return false;
    }

    private static Hit firstOk(List<Hit> hits) {
        for (Hit hit : hits) {
            if (hit.ok) {
                return hit;
            }
        }
        return null;
    }

    private static boolean contains(List<Hit> hits, String kind, String origin) {
        for (Hit hit : hits) {
            if (hit.kind.equals(kind) && hit.origin.equals(origin)) {
                return true;
            }
        }
        return false;
    }

    /** 把没探到的候选补成「跳过」行，逐行结果才和优先级表一致 */
    private static void fillMissing(List<Hit> hits, List<Group> groups, String reason) {
        for (Group group : groups) {
            for (String origin : group.origins) {
                if (contains(hits, group.kind, origin)) {
                    continue;
                }
                hits.add(new Hit(group.kind, origin, false, 0, reason, "", "", ""));
            }
        }
    }

    /** 收束：赢家 = 延迟最低的成功者；全挂时带上第一条失败原因 */
    private static Result finish(List<Hit> hits) {
        Result result = new Result();
        result.hits = new ArrayList<>(hits);
        Hit winner = null;
        for (Hit hit : hits) {
            if (hit.ok && (winner == null || hit.ms < winner.ms)) {
                winner = hit;
            }
        }
        if (winner == null) {
            for (Hit hit : hits) {
                if (hit.error != null && !hit.error.isEmpty()) {
                    result.error = "所有通道都不通：" + hit.error;
                    break;
                }
            }
            if (result.error.isEmpty()) {
                result.error = "所有通道都不通";
            }
            return result;
        }
        result.ok = true;
        result.kind = winner.kind == null ? "" : winner.kind;
        result.origin = winner.origin == null ? "" : winner.origin;
        result.ms = winner.ms;
        result.machineId = winner.machineId == null ? "" : winner.machineId;
        result.machineName = winner.machineName == null ? "" : winner.machineName;
        result.os = winner.os == null ? "" : winner.os;
        return result;
    }
}
