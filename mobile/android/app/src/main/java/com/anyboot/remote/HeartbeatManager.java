package com.anyboot.remote;

import android.app.Activity;
import android.os.Handler;
import android.os.Looper;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
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
 *   <li>一台电脑有多个候选地址时按 {@link ServerProfile#probeMode} 探：
 *       <b>并发（默认）</b>同探所有地址取延迟最低；<b>串行</b>按顺序探，第一个通的即止；</li>
 *   <li>上线<b>立即亮</b>；连续 {@link #OFFLINE_AFTER_FAILS} 轮全失败才判<b>离线</b>
 *       （防 WiFi 抖动误报）；</li>
 *   <li>只在 App 前台跑：onResume {@link #start()}（立即先探一轮），onPause {@link #stop()}；</li>
 *   <li>一轮没跑完不叠加下一轮（{@link #ticking} 闸 —— 串行多地址可能超过 10 秒）。</li>
 * </ul>
 *
 * <p>探测全部跑在后台线程；状态只在这里记账，变化时投递回主线程回调监听者。
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
    /** 单地址探测超时：比保存时的顺带探测宽容，弱网别误报离线 */
    static final int TIMEOUT_MS = 3000;
    /** 连续这么多轮全失败才判离线 */
    static final int OFFLINE_AFTER_FAILS = 2;
    /** 并发探测同时打出去的线程数 */
    private static final int PARALLEL_THREADS = 4;

    private final ScheduledExecutorService scheduler =
            Executors.newSingleThreadScheduledExecutor(r -> {
                Thread thread = new Thread(r, "anyboot-heartbeat");
                thread.setDaemon(true);
                return thread;
            });
    private final ExecutorService probePool =
            Executors.newFixedThreadPool(PARALLEL_THREADS, r -> {
                Thread thread = new Thread(r, "anyboot-heartbeat-ping");
                thread.setDaemon(true);
                return thread;
            });
    private final AtomicBoolean ticking = new AtomicBoolean(false);
    private final Map<String, State> states = new HashMap<>();
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
        // 上一轮还没跑完（串行多地址可能超过 10 秒）就不叠加
        if (!ticking.compareAndSet(false, true)) {
            return;
        }
        try {
            List<Pending> results = new ArrayList<>();
            for (ServerProfile profile : serversProvider.get()) {
                if (profile.addresses.isEmpty()) {
                    continue;
                }
                results.add(probeProfile(profile));
            }
            publish(results);
        } finally {
            ticking.set(false);
        }
    }

    /** 探一台电脑，返回「通没通 + 延迟」（后台线程） */
    private Pending probeProfile(ServerProfile profile) {
        // 经由跳板机的电脑：全部探测走 SSH 隧道（解析出的 127.0.0.1:端口）
        TunnelResolver resolver = tunnelResolver;
        if (!ServerProfile.text(profile.jumpServerId).isEmpty()) {
            if (resolver == null) {
                return new Pending(profile.id, -1);   // 解析器没接好：标「没探到」
            }
            try {
                String local = resolver.resolve(profile);
                long rtt = local == null || local.isEmpty()
                        ? -1 : HealthProbe.ping(local, TIMEOUT_MS);
                return new Pending(profile.id, rtt);
            } catch (Exception ignored) {
                return new Pending(profile.id, -1);
            }
        }
        List<String> addresses = new ArrayList<>(profile.addresses);
        if (ServerProfile.PROBE_SERIAL.equals(profile.probeMode)) {
            // 串行：按顺序逐个探，第一个通的即止（保留用户排的地址优先级）
            for (String address : addresses) {
                long rtt = HealthProbe.ping(address, TIMEOUT_MS);
                if (rtt >= 0) {
                    return new Pending(profile.id, rtt);
                }
            }
            return new Pending(profile.id, -1);
        }
        // 并发：所有地址同时探，取 RTT 最低的当「这台电脑现在的延迟」
        CountDownLatch latch = new CountDownLatch(addresses.size());
        final Best best = new Best();
        for (String address : addresses) {
            probePool.execute(() -> {
                try {
                    long rtt = HealthProbe.ping(address, TIMEOUT_MS);
                    if (rtt >= 0) {
                        best.offer(rtt);
                    }
                } finally {
                    latch.countDown();
                }
            });
        }
        try {
            // 兜底等一眼：正常情况下每个 ping 自己 3 秒就超时了
            latch.await(30, TimeUnit.SECONDS);
        } catch (InterruptedException ignored) {
            Thread.currentThread().interrupt();
        }
        return new Pending(profile.id, best.rtt);
    }

    /** 把一轮结果落账；有变化投递回主线程（后台线程） */
    private void publish(List<Pending> results) {
        if (results.isEmpty()) {
            return;
        }
        final List<String> wentOnline = new ArrayList<>();
        boolean anyResult = false;
        synchronized (states) {
            for (Pending pending : results) {
                State state = states.get(pending.id);
                if (state == null) {
                    state = new State();
                    states.put(pending.id, state);
                }
                anyResult = true;
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
        });
    }

    /** 一台电脑的一轮探测结果：rtt >= 0 = 通（并发模式下已取最快地址的 RTT） */
    private static final class Pending {
        final String id;
        final long rtt;

        Pending(String id, long rtt) {
            this.id = id;
            this.rtt = rtt;
        }
    }

    /** 并发探测里挑最低 RTT 的小账本 */
    private static final class Best {
        private long rtt = -1;

        synchronized void offer(long candidate) {
            if (rtt < 0 || candidate < rtt) {
                rtt = candidate;
            }
        }
    }
}
