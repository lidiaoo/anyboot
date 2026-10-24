package com.anyboot.remote;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.ThreadFactory;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * **按「连接策略」探测**：通道优先级（route_order）+ 探测方式（并发首个 / 并发全部 / 串行）
 * + 探测超时（probeTimeoutMs），与 PC 端 `probe_candidates_blocking` 同一套语义。
 *
 * <p>为什么要有这一层：以前「电脑」页与「电脑列表」各写一套探活 —— 列表只看局域网地址，
 * 主页面才认远程通道，于是同一台电脑在两个页面的在线状态能不一致；探测方式也只作用于
 * 多网卡。现在两页共用这里的结果：<b>谁在前先试谁、并发还是串行听用户的、超时也听用户的</b>。
 *
 * <p>线程池是**探测专用**的（{@link #executor}）：心跳每 10 秒一轮，弱网/离线时每个地址
 * 都要等满超时，如果和隧道、JSON 请求抢同一个池，界面会跟着卡。池子固定 4~8 个线程，
 * 且**只跑探测**（`/health` 这种只读短请求）。
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
            return "tunnel".equals(kind) || "ts".equals(kind)
                    || "funnel".equals(kind) || "tsnet".equals(kind);
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
        java.util.Map<String, Integer> timeouts = new java.util.HashMap<>();
        for (String[] candidate : candidates) {
            timeouts.put(candidate[0], profile.probeTimeoutFor(candidate[0]));
        }
        String mode = ServerProfile.normalizeProbeMode(profile.probeMode);
        if (ServerProfile.PROBE_SERIAL.equals(mode)) {
            return serial(candidates, timeouts, jump);
        }
        return parallel(candidates, timeouts, jump, ServerProfile.PROBE_PARALLEL_ALL.equals(mode));
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

    /** 串行：按优先级逐个试，第一个通的就是赢家（省流量，最慢） */
    private static Result serial(List<String[]> candidates,
                                 java.util.Map<String, Integer> timeouts, JumpResolver jump) {
        List<Hit> hits = new ArrayList<>();
        for (String[] candidate : candidates) {
            Hit hit = probeOne(candidate[0], candidate[1], timeoutOf(timeouts, candidate[0]), jump);
            hits.add(hit);
            if (hit.ok) {
                return finish(hits, hit);
            }
        }
        return finish(hits, null);
    }

    /** 这个通道该等多久（毫秒）；表里没有就按默认 2 秒兜底（正常情况下表里一定有） */
    private static int timeoutOf(java.util.Map<String, Integer> timeouts, String kind) {
        Integer value = timeouts.get(kind);
        return value == null || value <= 0
                ? ServerProfile.DEFAULT_PROBE_TIMEOUT_MS
                : ServerProfile.clampProbeTimeoutMs(value);
    }

    /**
     * 并发：**全部候选同时发出**。`all=true` 等全部返回取最快成功者；
     * `all=false`（并发首个）第一个成功立刻返回，其余在途任务结果丢弃。
     */
    private static Result parallel(List<String[]> candidates,
                                   java.util.Map<String, Integer> timeouts,
                                   JumpResolver jump, boolean all) {
        final int expected = candidates.size();
        // 逐行结果按**候选顺序**摆回数组（哪个线程先回来都不影响展示顺序）
        final Hit[] ordered = new Hit[expected];
        // 谁先回来谁先进队列 —— 必须按「完成先后」取，不能按提交顺序 get，
        // 否则「并发·首个」会先干等第一个死通道超时，后面的快通道赢了也白等。
        final BlockingQueue<Hit> arrived = new LinkedBlockingQueue<>();
        int slowest = 0;
        List<Future<?>> futures = new ArrayList<>();
        for (int index = 0; index < expected; index++) {
            final String kind = candidates.get(index)[0];
            final String origin = candidates.get(index)[1];
            final int timeout = timeoutOf(timeouts, kind);
            slowest = Math.max(slowest, timeout);
            final int slot = index;
            futures.add(Holder.EXECUTOR.submit(new Runnable() {
                @Override
                public void run() {
                    Hit hit = probeOne(kind, origin, timeout, jump);
                    ordered[slot] = hit;
                    arrived.offer(hit);
                }
            }));
        }
        Hit winner = null;
        // 兜底时限按**最慢那个通道**的超时算，否则会被第一个通道的短超时把慢通道提前拍死
        long deadline = System.currentTimeMillis() + slowest + 500L;
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
                break;   // 到点了：还没回来的都按超时记
            }
            if (hit.ok && (winner == null || hit.ms < winner.ms)) {
                winner = hit;
                if (!all) {
                    // 并发首个：已经有人通了 —— 剩下的取消掉，不再白等它们超时
                    for (Future<?> future : futures) {
                        future.cancel(true);
                    }
                    break;
                }
            }
        }
        List<Hit> hits = new ArrayList<>(expected);
        for (int index = 0; index < expected; index++) {
            Hit hit = ordered[index];
            hits.add(hit != null ? hit : new Hit(
                    candidates.get(index)[0], candidates.get(index)[1], false, 0,
                    "探测超时（>" + timeoutOf(timeouts, candidates.get(index)[0]) + "ms）",
                    "", "", ""));
        }
        return finish(hits, winner);
    }

    /** 探一个候选：局域网 → http://host:port/health；远程 → https origin + /health */
    private static Hit probeOne(String kind, String origin, int timeout, JumpResolver jump) {
        boolean remote = isRemote(kind);
        String target = origin;
        String error = "";
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
            error = "连不上（超时或对方没起 HTTP 服务）";
            return new Hit(kind, target, false, 0, error, "", "", "");
        }
        return new Hit(kind, target, true, probe.ms, "", probe.machineId, probe.machineName, probe.os);
    }

    private static boolean isRemote(String kind) {
        return "tunnel".equals(kind) || "ts".equals(kind)
                || "funnel".equals(kind) || "tsnet".equals(kind);
    }

    private static Result finish(List<Hit> hits, Hit winner) {
        Result result = new Result();
        result.hits = new ArrayList<>(hits);
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
        result.kind = winner.kind;
        result.origin = winner.origin;
        result.ms = winner.ms;
        result.machineId = winner.machineId == null ? "" : winner.machineId;
        result.machineName = winner.machineName == null ? "" : winner.machineName;
        result.os = winner.os == null ? "" : winner.os;
        return result;
    }
}
