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
        /** 这条结果属于哪条配对（局域网 / 跳板机为空）—— 前端按它回填「哪一行的状态」 */
        final String pairId;
        final boolean ok;
        final long ms;
        final String error;
        final String machineId;
        final String machineName;
        final String os;

        Hit(String kind, String origin, String pairId, boolean ok, long ms, String error,
            String machineId, String machineName, String os) {
            this.kind = kind;
            this.origin = origin;
            this.pairId = pairId == null ? "" : pairId;
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
        /** 赢家是哪条配对（局域网 / 跳板机为空）—— 调用方按它取那条自己的令牌 */
        String pairId = "";
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

    /**
     * 一个通道里的一个候选。超时与配对 id 在展开时就按条目算好（三层继承的实现只有一处），
     * 探测时不用再回头查表 —— 同一个通道里的三条配对可以有三种超时。
     */
    private static final class Item {
        final String origin;
        final int timeoutMs;
        final String pairId;

        Item(String origin, int timeoutMs, String pairId) {
            this.origin = origin;
            this.timeoutMs = timeoutMs;
            this.pairId = pairId == null ? "" : pairId;
        }
    }

    /**
     * 一个通道的候选组。
     *
     * <p>{@code innerMode} 是该组内部的探测策略：局域网 / 跳板机用档案的多网卡策略
     * （{@link ServerProfile#probeMode}），配对通道用**区块内**策略
     * （{@link ServerProfile#blockProbeModeFor}）—— 同一区块里的多条配对之间怎么试，
     * 与「一块网卡多个 IP 之间怎么试」是两件事。
     */
    private static final class Group {
        final String kind;
        final String innerMode;
        final List<Item> items = new ArrayList<>();

        Group(String kind, String innerMode) {
            this.kind = kind;
            this.innerMode = innerMode;
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
        List<ServerProfile.RouteCandidate> candidates = expand(profile, onlyKinds);
        if (candidates.isEmpty()) {
            // 全通道路径要报错给用户看；限定通道的阶段性探测「没有候选」是正常情况
            result.error = onlyKinds == null ? "没有可用连接通道（地址为空，或通道都被停用）" : "";
            return result;
        }
        List<Group> groups = group(profile, candidates);
        String routeMode = ServerProfile.normalizeProbeMode(profile.routeProbeMode);
        return ServerProfile.PROBE_SERIAL.equals(routeMode)
                ? probeChannelsSerial(groups, jump)
                : probeChannelsParallel(groups, jump,
                        ServerProfile.PROBE_PARALLEL_ALL.equals(routeMode));
    }

    /**
     * routeCandidates 里 lan 只占一条（请求路径用），探测要**每个局域网地址各一条**，
     * 否则多网卡的机器永远只探第一块网卡。
     */
    private static List<ServerProfile.RouteCandidate> expand(ServerProfile profile, List<String> onlyKinds) {
        List<ServerProfile.RouteCandidate> out = new ArrayList<>();
        for (ServerProfile.RouteCandidate candidate : profile.routeCandidates()) {
            String kind = candidate.kind;
            if (onlyKinds != null && !onlyKinds.contains(kind)) {
                continue;
            }
            if (ServerProfile.ROUTE_LAN.equals(kind)) {
                int timeout = profile.probeTimeoutFor(kind);
                for (String address : profile.addresses) {
                    String trimmed = ServerProfile.text(address);
                    if (!trimmed.isEmpty()) {
                        out.add(new ServerProfile.RouteCandidate(kind, trimmed, "", timeout, ""));
                    }
                }
            } else {
                out.add(candidate);
            }
        }
        return out;
    }

    /** 按 kind 分组，**保持候选出现顺序**（组顺序 = 用户在优先级表里排的顺序） */
    private static List<Group> group(ServerProfile profile, List<ServerProfile.RouteCandidate> candidates) {
        List<Group> groups = new ArrayList<>();
        for (ServerProfile.RouteCandidate candidate : candidates) {
            Group hit = null;
            for (Group group : groups) {
                if (group.kind.equals(candidate.kind)) {
                    hit = group;
                    break;
                }
            }
            if (hit == null) {
                String block = RemotePair.blockOfRoute(candidate.kind);
                String inner = block.isEmpty()
                        ? profile.probeMode
                        : profile.blockProbeModeFor(block);
                hit = new Group(candidate.kind, ServerProfile.normalizeProbeMode(inner));
                groups.add(hit);
            }
            // 同一个 origin 只探一次；不同配对即使地址相同也各占一行（令牌不同 = 另一条路）
            boolean seen = false;
            for (Item item : hit.items) {
                if (item.origin.equals(candidate.origin) && item.pairId.equals(candidate.pairId)) {
                    seen = true;
                    break;
                }
            }
            if (!seen) {
                hit.items.add(new Item(candidate.origin, candidate.timeoutMs, candidate.pairId));
            }
        }
        return groups;
    }

    /** 这组最坏等多久（内层串行时＝逐个累加，否则取最长的一条） */
    private static int worstOf(Group group) {
        if (ServerProfile.PROBE_SERIAL.equals(group.innerMode)) {
            int sum = 0;
            for (Item item : group.items) {
                sum += item.timeoutMs;
            }
            return sum;
        }
        int max = 0;
        for (Item item : group.items) {
            max = Math.max(max, item.timeoutMs);
        }
        return max;
    }

    /** 外层串行：一个通道一个通道地试，某个通道命中就不再往下走 */
    private static Result probeChannelsSerial(List<Group> groups, JumpResolver jump) {
        List<Hit> hits = new ArrayList<>();
        for (Group group : groups) {
            List<Hit> groupHits = probeGroup(group, jump);
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
     * 外层并发：一个通道一个任务，组内再按内层策略处理它的多个候选。
     * `all=true` 等全部通道返回取最快成功者；`all=false` 第一个成功的通道立刻返回。
     */
    private static Result probeChannelsParallel(List<Group> groups,
                                                JumpResolver jump, boolean all) {
        final int expected = groups.size();
        final List<List<Hit>> ordered = new ArrayList<>(Collections.nCopies(expected, null));
        // 谁先回来谁先进队列 —— 必须按「完成先后」取，不能按提交顺序等
        final BlockingQueue<int[]> arrived = new LinkedBlockingQueue<>();
        int slowest = 0;
        for (int index = 0; index < expected; index++) {
            final Group group = groups.get(index);
            final int slot = index;
            final JumpResolver resolver = jump;
            slowest = Math.max(slowest, worstOf(group));
            Holder.EXECUTOR.execute(new Runnable() {
                @Override
                public void run() {
                    List<Hit> groupHits = probeGroup(group, resolver);
                    // 按槽位摆回去（展示顺序 = 优先级顺序），再报告「通道完成」
                    synchronized (ordered) {
                        ordered.set(slot, groupHits);
                    }
                    arrived.offer(new int[]{slot});
                }
            });
        }
        // 30 秒硬顶：超时下放到条目后最坏耗时是乘出来的（3 区块 × 4 条 × 3s = 36s），
        // 不封顶的话「探测」按钮能转一分钟
        long budget = Math.min(slowest + 500L, ServerProfile.MAX_PROBE_TIMEOUT_MS);
        long deadline = System.currentTimeMillis() + budget;
        // 严格区分两种「没回来」：赢家已出、在途通道被并发首个掐掉 = 未测出（不是不通）；
        // 真跑到超时还没回 = 探测超时
        boolean wonEarly = false;
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
                wonEarly = true;
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
        // 没回来的通道 / 候选如实标出来（带上它自己的超时值）
        for (Group group : groups) {
            for (Item item : group.items) {
                if (contains(hits, group.kind, item.origin, item.pairId)) {
                    continue;
                }
                hits.add(new Hit(group.kind, item.origin, item.pairId, false, 0,
                        wonEarly ? "已有通道命中，未测出"
                                : "探测超时（>" + item.timeoutMs + "ms）",
                        "", "", ""));
            }
        }
        return finish(hits);
    }

    /**
     * 探一个通道：组内多个候选按**该组的内部策略**处理，返回按 items 顺序排列的命中。
     * 未探测的候选也补一行，这样逐行结果永远和优先级表对得上。
     */
    private static List<Hit> probeGroup(Group group, JumpResolver jump) {
        List<Hit> hits = new ArrayList<>();
        if (group.items.size() == 1) {
            Item item = group.items.get(0);
            hits.add(probeOne(group.kind, item.origin, item.timeoutMs, item.pairId, jump));
            return hits;
        }
        if (ServerProfile.PROBE_SERIAL.equals(group.innerMode)) {
            // 内层串行：按用户排的顺序逐个试，第一个通就停
            for (Item item : group.items) {
                Hit hit = probeOne(group.kind, item.origin, item.timeoutMs, item.pairId, jump);
                hits.add(hit);
                if (hit.ok) {
                    break;
                }
            }
            for (int index = hits.size(); index < group.items.size(); index++) {
                Item item = group.items.get(index);
                hits.add(new Hit(group.kind, item.origin, item.pairId, false, 0,
                        "同通道前面已命中，未探测", "", "", ""));
            }
            return hits;
        }
        // 内层并发：全部同时发出，按完成先后取（首个成功即收工 / 等全部）
        final int expected = group.items.size();
        final Hit[] ordered = new Hit[expected];
        final BlockingQueue<Hit> arrived = new LinkedBlockingQueue<>();
        final boolean all = ServerProfile.PROBE_PARALLEL_ALL.equals(group.innerMode);
        List<Future<?>> futures = new ArrayList<>();
        for (int index = 0; index < expected; index++) {
            final Item item = group.items.get(index);
            final String kind = group.kind;
            final int slot = index;
            final JumpResolver resolver = jump;
            futures.add(Holder.EXECUTOR.submit(new Runnable() {
                @Override
                public void run() {
                    Hit hit = probeOne(kind, item.origin, item.timeoutMs, item.pairId, resolver);
                    ordered[slot] = hit;
                    arrived.offer(hit);
                }
            }));
        }
        long budget = Math.min(worstOf(group) + 500L, ServerProfile.MAX_PROBE_TIMEOUT_MS);
        long deadline = System.currentTimeMillis() + budget;
        // 严格区分：赢家已出、在途被掐 = 未测出；等满时限没回 = 探测超时
        boolean wonEarly = false;
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
                wonEarly = true;
                for (Future<?> future : futures) {
                    future.cancel(true);
                }
                break;
            }
        }
        for (int index = 0; index < expected; index++) {
            Hit hit = ordered[index];
            Item item = group.items.get(index);
            hits.add(hit != null ? hit : new Hit(group.kind, item.origin, item.pairId, false, 0,
                    wonEarly ? "同通道已命中，未测出"
                            : "探测超时（>" + item.timeoutMs + "ms）",
                    "", "", ""));
        }
        return hits;
    }

    /** 探一个候选：局域网 → http://host:port/health；远程 → https origin + /health */
    private static Hit probeOne(String kind, String origin, int timeout, String pairId, JumpResolver jump) {
        boolean remote = isRemote(kind);
        String target = origin;
        if (ServerProfile.ROUTE_JUMP.equals(kind)) {
            if (jump == null) {
                return new Hit(kind, "", pairId, false, 0, "没配跳板机解析器", "", "", "");
            }
            try {
                target = jump.resolve();
            } catch (Exception exception) {
                return new Hit(kind, "", pairId, false, 0, "跳板机不可用：" + exception.getMessage(), "", "", "");
            }
            if (target == null || target.trim().isEmpty()) {
                return new Hit(kind, "", pairId, false, 0, "跳板机隧道没建起来", "", "", "");
            }
            remote = false;
        }
        HealthProbe.Result probe = HealthProbe.probe(target, remote, timeout);
        if (probe == null) {
            return new Hit(kind, target, pairId, false, 0,
                    "连不上（超时或对方没起 HTTP 服务）", "", "", "");
        }
        return new Hit(kind, target, pairId, true, probe.ms,
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

    private static boolean contains(List<Hit> hits, String kind, String origin, String pairId) {
        String id = pairId == null ? "" : pairId;
        for (Hit hit : hits) {
            if (hit.kind.equals(kind) && hit.origin.equals(origin) && hit.pairId.equals(id)) {
                return true;
            }
        }
        return false;
    }

    /** 把没探到的候选补成「跳过」行，逐行结果才和优先级表一致 */
    private static void fillMissing(List<Hit> hits, List<Group> groups, String reason) {
        for (Group group : groups) {
            for (Item item : group.items) {
                if (contains(hits, group.kind, item.origin, item.pairId)) {
                    continue;
                }
                hits.add(new Hit(group.kind, item.origin, item.pairId, false, 0, reason, "", "", ""));
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
        result.pairId = winner.pairId == null ? "" : winner.pairId;
        result.ms = winner.ms;
        result.machineId = winner.machineId == null ? "" : winner.machineId;
        result.machineName = winner.machineName == null ? "" : winner.machineName;
        result.os = winner.os == null ? "" : winner.os;
        return result;
    }
}
