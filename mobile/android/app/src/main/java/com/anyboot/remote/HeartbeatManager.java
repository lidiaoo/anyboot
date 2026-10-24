package com.anyboot.remote;

import android.os.Handler;
import android.os.Looper;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * 10 秒心跳：轮询全部服务端条目的 {@code /health}，维护「在线 / 离线」状态，
 * 给顶部标签的状态点用（在线=绿、离线=灰；加载中的黄点仍由 WebView 报）。
 *
 * <p>规则（与 docs/client-todo.md F1 一致，iOS 端 Heartbeat.swift 同构）：
 * <ul>
 *   <li>探测<b>完全按档案的连接策略</b>（{@link ProbeEngine}）：通道优先级
 *       {@code route_order}（局域网 / 跳板 / 隧道 / Tailscale / Funnel / 内置 tsnet）、
 *       探测方式（并发首个 / 并发全部 / 串行）、探测超时 {@code probeTimeoutMs}
 *       全部听用户的 —— 与「电脑列表」的在线判断、与真正连接走的通道是同一套语义；</li>
 *   <li>省电两段式：通道优先级是「本地优先」时，先只探局域网 / 跳板机，
 *       通了就到此为止（不惊动公网）；本地全挂才兜底探远程，且最多
 *       {@link #REMOTE_FALLBACK_INTERVAL_MS} 打一次公网；</li>
 *   <li>上线<b>立即亮</b>；连续 {@link #OFFLINE_AFTER_FAILS} 轮全失败才判<b>离线</b>
 *       （防 WiFi 抖动误报）；</li>
 *   <li>只在 App 前台跑：onResume {@link #start()}（立即先探一轮），onPause {@link #stop()}；</li>
 *   <li>一轮没跑完不叠加下一轮（{@link #ticking} 闸 —— 串行多通道可能超过 10 秒）。</li>
 * </ul>
 *
 * <p>探测全部跑在后台线程（{@link ProbeEngine} 的专用池，不和隧道 / JSON 抢线程）；
 * 状态只在这里记账，变化时投递回主线程回调监听者。
 */
final class HeartbeatManager {

    /** 提供当前的服务端列表（取快照，别让心跳线程和 UI 同时改同一份） */
    interface ServersProvider {
        List<ServerProfile> get();
    }

    /**
     * 「经由跳板机」的电脑：把目标内网地址解析成本地隧道地址
     * （{@code 127.0.0.1:端口}）。在心跳后台线程调用；失败抛异常（判离线）。
     * 由 MainActivity 注入（因为要动 SshSessionManager）。
     */
    interface TunnelResolver {
        String resolve(ServerProfile profile) throws Exception;
    }

    interface Listener {
        /** 心跳状态有变化（已在主线程）——刷标签状态点 */
        void onHeartbeatChanged();

        /** 某台电脑从「非在线」变「在线」（已在主线程）——错误面板自愈重连用 */
        void onWentOnline(String id);

        /**
         * 某台电脑的<b>局域网路径</b>恢复可达（上一轮还不通；已在主线程）——
         * 还挂在远程隧道上的标签可以借此自动切回直连。
         */
        default void onLanRecovered(String id) {
        }

        /**
         * 探到了这台电脑的机器标识 / 系统族，且与上次不同（已在主线程）——
         * 宿主记下来（下次开界面就能直接画系统徽标，不用等一轮探测）。
         *
         * @param os windows / macos / linux；对方是老版本服务端时为空串
         */
        default void onMachineSeen(String id, String machineId, String machineName, String os) {
        }
    }

    /** 一台电脑的心跳状态（快照给 UI 读，字段只在这里改） */
    static final class State {
        /** 有效在线（离线要等连续失败凑满才翻 false） */
        private boolean online;
        long rttMs;
        long lastSeen;
        int failures;

        public boolean isOnline() {
            return online;
        }

        /** 连续全失败凑满 {@link #OFFLINE_AFTER_FAILS} 轮才算离线 */
        public boolean isOffline() {
            return !online && failures >= OFFLINE_AFTER_FAILS;
        }
    }

    static final long INTERVAL_MS = 10000;
    /** 连续这么多轮全失败才判离线 */
    static final int OFFLINE_AFTER_FAILS = 2;

    /**
     * 远程兜底探测的降频间隔：局域网全挂（出门是常态）时才探远程，
     * 但最多每 60 秒打一次公网 —— 每 10 秒一轮心跳都往隧道发请求既费电又没意义。
     */
    static final long REMOTE_FALLBACK_INTERVAL_MS = 60000;

    private final ScheduledExecutorService scheduler =
            Executors.newSingleThreadScheduledExecutor(r -> {
                Thread thread = new Thread(r, "anyboot-heartbeat");
                thread.setDaemon(true);
                return thread;
            });
    private final AtomicBoolean ticking = new AtomicBoolean(false);
    private final Map<String, State> states = new HashMap<>();
    /** 远程兜底探测的节流账本（profile.id → 上次探远程的时间戳；只关心新旧，别当精确时间） */
    private final Map<String, Long> remoteProbes = new HashMap<>();
    /** 局域网路径上轮通没通（profile.id → Boolean）：恢复沿要给「远程切回局域网」用 */
    private final Map<String, Boolean> lanFlags = new HashMap<>();
    /** 上轮上报过的机器指纹（profile.id → "机器标识\u0000机器名\u0000系统族"）：变了才回调 */
    private final Map<String, String> machineFlags = new HashMap<>();
    private final Handler mainHandler = new Handler(Looper.getMainLooper());
    private final ServersProvider serversProvider;
    private final Listener listener;
    /** 走跳板机探测的解析器（MainActivity 注入；null = 还没接好，隧道条目不探） */
    volatile TunnelResolver tunnelResolver;
    private ScheduledFuture<?> future;

    HeartbeatManager(ServersProvider serversProvider, Listener listener) {
        this.serversProvider = serversProvider;
        this.listener = listener;
    }

    // ------------------------------------------------------------------
    // 前台 / 后台
    // ------------------------------------------------------------------

    /** 回前台：立即先探一轮，并按 10 秒固定间隔排下去 */
    void start() {
        stop();
        future = scheduler.scheduleWithFixedDelay(this::tickSafe, 0, INTERVAL_MS, TimeUnit.MILLISECONDS);
    }

    /** 退后台：停表（省电；回前台 start() 会先补探一轮） */
    void stop() {
        if (future != null) {
            future.cancel(false);
            future = null;
        }
    }

    // ------------------------------------------------------------------
    // 读取
    // ------------------------------------------------------------------

    /** 这台电脑的心跳状态快照；没探过给 null */
    State stateOf(String id) {
        synchronized (states) {
            State state = states.get(id);
            if (state == null) {
                return null;
            }
            State snapshot = new State();
            snapshot.online = state.online;
            snapshot.rttMs = state.rttMs;
            snapshot.lastSeen = state.lastSeen;
            snapshot.failures = state.failures;
            return snapshot;
        }
    }

    // ------------------------------------------------------------------
    // 一轮探测
    // ------------------------------------------------------------------

    private void tickSafe() {
        try {
            tick();
        } catch (Exception ignored) {
            // 调度线程上抛异常会把整条周期任务杀掉，这里必须兜住
        }
    }

    private void tick() {
        // 上一轮还没跑完（串行多通道可能超过 10 秒）就不叠加
        if (!ticking.compareAndSet(false, true)) {
            return;
        }
        try {
            List<Pending> results = new ArrayList<>();
            for (ServerProfile profile : serversProvider.get()) {
                // 没配局域网地址也可能配了远程接入（出门场景）：空档案才跳过
                if (profile.addresses.isEmpty() && !profile.hasRemote()) {
                    continue;
                }
                results.add(probeProfile(profile));
            }
            publish(results);
        } finally {
            ticking.set(false);
        }
    }

    /**
     * 探一台电脑，返回「通没通 + 延迟 + 系统族」（后台线程）。
     *
     * <p>完全交给 {@link ProbeEngine}：通道优先级、并发/串行、超时都按档案策略走。
     * 唯一的额外规则是「远程降频」—— 本地优先时先去探本地，本地全挂才看公网，
     * 且 60 秒内只打一次（心跳 10 秒一轮，不节流会把隧道打成筛子）。
     */
    private Pending probeProfile(ServerProfile profile) {
        TunnelResolver resolver = tunnelResolver;
        ProbeEngine.JumpResolver jump = resolver == null
                ? null
                : () -> resolver.resolve(profile);
        boolean remoteFirst = profile.routePriorityRemoteFirst();
        if (!remoteFirst) {
            ProbeEngine.Result local = ProbeEngine.probe(profile, jump, ProbeEngine.LOCAL_KINDS);
            if (local.ok || !profile.hasRemote()) {
                return new Pending(profile.id, local);
            }
            if (!remoteDue(profile.id)) {
                // 还没到点：这轮按「本地不通」记（与旧行为一致），下轮或 60 秒后再看公网
                return new Pending(profile.id, local);
            }
        }
        return new Pending(profile.id, ProbeEngine.probe(profile, jump));
    }

    /** 远程兜底探测的节流闸：到点了就记账并返回 true */
    private boolean remoteDue(String id) {
        long now = System.currentTimeMillis();
        synchronized (remoteProbes) {
            Long last = remoteProbes.get(id);
            if (last != null && now - last < REMOTE_FALLBACK_INTERVAL_MS) {
                return false;
            }
            remoteProbes.put(id, now);
            return true;
        }
    }

    /** 把一轮结果落账；有变化投递回主线程（后台线程） */
    private void publish(List<Pending> results) {
        if (results.isEmpty()) {
            return;
        }
        final List<String> wentOnline = new ArrayList<>();
        final List<String> lanRecovered = new ArrayList<>();
        final List<Pending> machines = new ArrayList<>();
        boolean anyResult = false;
        synchronized (states) {
            for (Pending pending : results) {
                State state = states.get(pending.id);
                if (state == null) {
                    state = new State();
                    states.put(pending.id, state);
                }
                anyResult = true;
                // 局域网路径恢复沿：上轮不通、这轮通了（与总在线无关 —— 远程兜着也算离线态）
                boolean lanOkBefore = Boolean.TRUE.equals(lanFlags.get(pending.id));
                boolean lanOkNow = pending.lanRtt >= 0;
                lanFlags.put(pending.id, lanOkNow);
                if (lanOkNow && !lanOkBefore) {
                    lanRecovered.add(pending.id);
                }
                if (pending.rtt >= 0) {
                    state.failures = 0;
                    state.rttMs = pending.rtt;
                    state.lastSeen = System.currentTimeMillis();
                    if (!state.online) {
                        state.online = true;
                        wentOnline.add(pending.id);
                    }
                } else {
                    state.failures++;
                    // 连续失败凑满才翻成离线 —— 前几轮维持原状态，别闪
                    if (state.failures >= OFFLINE_AFTER_FAILS && state.online) {
                        state.online = false;
                    }
                }
                if (pending.hasMachine() && machineChanged(pending)) {
                    machines.add(pending);
                }
            }
        }
        if (!anyResult) {
            return;
        }
        mainHandler.post(() -> {
            listener.onHeartbeatChanged();
            for (String id : wentOnline) {
                listener.onWentOnline(id);
            }
            for (String id : lanRecovered) {
                listener.onLanRecovered(id);
            }
            for (Pending pending : machines) {
                listener.onMachineSeen(pending.id, pending.machineId, pending.machineName, pending.os);
            }
        });
    }

    /** 机器指纹变了没有（变了才回调宿主，别每 10 秒都写一次盘） */
    private boolean machineChanged(Pending pending) {
        String fingerprint = pending.machineId + "\u0000" + pending.machineName + "\u0000" + pending.os;
        String last;
        synchronized (machineFlags) {
            last = machineFlags.get(pending.id);
            if (fingerprint.equals(last)) {
                return false;
            }
            machineFlags.put(pending.id, fingerprint);
        }
        return true;
    }

    /**
     * 一台电脑的一轮探测结果：rtt = 总口径（远程兜底也算）；
     * lanRtt = 局域网路径单独记（恢复沿用）；机器标识 / 系统族来自赢家通道的 /health。
     */
    private static final class Pending {
        final String id;
        final long rtt;
        final long lanRtt;
        final String machineId;
        final String machineName;
        final String os;

        Pending(String id, ProbeEngine.Result result) {
            this.id = id;
            this.rtt = result != null && result.ok ? result.ms : -1;
            this.lanRtt = lanRttOf(result);
            this.machineId = result == null ? "" : ServerProfile.text(result.machineId);
            this.machineName = result == null ? "" : ServerProfile.text(result.machineName);
            this.os = result == null ? "" : ServerProfile.normalizeMachineOs(result.os);
        }

        boolean hasMachine() {
            return !machineId.isEmpty() || !machineName.isEmpty() || !os.isEmpty();
        }

        /** 局域网路径的最好成绩（没探 / 全挂 → -1） */
        private static long lanRttOf(ProbeEngine.Result result) {
            if (result == null || result.hits == null) {
                return -1;
            }
            long best = -1;
            for (ProbeEngine.Hit hit : result.hits) {
                if (!hit.ok || !ServerProfile.ROUTE_LAN.equals(hit.kind)) {
                    continue;
                }
                if (best < 0 || hit.ms < best) {
                    best = hit.ms;
                }
            }
            return best;
        }
    }
}
