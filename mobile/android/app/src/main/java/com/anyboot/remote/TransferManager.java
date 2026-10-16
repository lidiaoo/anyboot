package com.anyboot.remote;

import android.content.Context;
import android.content.res.AssetFileDescriptor;
import android.net.Uri;

import com.jcraft.jsch.ChannelSftp;
import com.jcraft.jsch.Session;
import com.jcraft.jsch.SftpProgressMonitor;

import java.io.File;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;

/**
 * 传输中心：手机 ⇄ 服务器 的文件上传 / 下载任务注册表（静态持有，切页与 Activity 重建都不丢）。
 *
 * <p>「文件页」只负责发起，进度、暂停、继续、取消、重试、结果都记在这里，「传输页」展示这份清单。
 * 传输复用 {@link SshSessionManager} 那条共享 SSH 会话上另开的 sftp 通道 —— 与终端、
 * 隧道互不影响。
 *
 * <p><b>取消 / 暂停的实现依据</b>（已核过 jsch 0.2.20 的 {@code ChannelSftp} 源码）：
 * <ul>
 *   <li>{@link SftpProgressMonitor#count(long)} 收到的是**本块新增字节数**（不是累计值），
 *       返回 {@code false} 时 put/get 的就地循环直接 break，剩余数据被 skip 掉后正常收尾 ——
 *       所以停下来既不会抛异常，也不用去断共享会话。</li>
 *   <li>两者的区别只在**半截文件留不留**：取消清掉，暂停留着当续传起点。</li>
 * </ul>
 *
 * <p><b>续传</b>不用自己写断点逻辑，jsch 原生带：
 * <ul>
 *   <li>下载 {@code get(src, dst, monitor, ChannelSftp.RESUME)}：自己 {@code _stat} 远端拿总长、
 *       比对本地文件长度（本地更大直接报错、相等直接返回），然后以追加方式打开本地文件、
 *       从断点开始 READ。</li>
 *   <li>上传 {@code put(input, dst, monitor, ChannelSftp.RESUME)}：{@code _stat} 远端拿已传长度，
 *       {@code src.skip(已传)} 跳过本地已传部分，再以 APPEND 打开远端、从该偏移续写。</li>
 * </ul>
 * 注意两者的**进度起点上报方式不一样**：下载时 jsch 会紧跟在 {@code init} 后补一次
 * {@code count(已下字节)}（那一笔只能当基线，不能当本次进度），上传时**不会**报，
 * 得自己先 {@code lstat} 拿远端长度当起点 —— 这个差异在 {@link Progress} 里用
 * {@code swallowFirstCount} 与 {@code initialTransferred} 分开处理，改动这里时务必注意。
 *
 * <p>已知边界：任务表是进程内的静态结构，**杀掉进程后暂停中的记录会丢**（半截文件还在，
 * 但没人认领它）。跨进程续传要先做任务持久化，是另一件事。
 */
final class TransferManager {

    // ---- 方向 ----
    static final int DIR_UPLOAD = 1;
    static final int DIR_DOWNLOAD = 2;

    // ---- 状态 ----
    static final int STATE_RUNNING = 0;
    static final int STATE_DONE = 1;
    static final int STATE_FAILED = 2;
    static final int STATE_CANCELLED = 3;
    /** 暂停：人主动停的，半截文件留着，可以「继续」接着传 */
    static final int STATE_PAUSED = 4;
    /** 已被「继续」接替：新任务接管了断点，这条只留个说法，不再有可点的动作 */
    static final int STATE_SUPERSEDED = 5;

    // ---- 停止意图：传输线程只看这一个字段 ----
    private static final int STOP_NONE = 0;
    /** 取消：半截文件清掉 */
    private static final int STOP_CANCEL = 1;
    /** 暂停：半截文件留着当续传起点 */
    private static final int STOP_PAUSE = 2;

    /** 已完成任务最多留这么多条（再老的就丢掉，省内存） */
    private static final int FINISHED_LIMIT = 30;

    interface Observer {
        /** 任务集合或进度有变 —— 传输页刷新 */
        void onTransfersChanged();
    }

    /** 一条传输任务（字段基本都在传输线程与 UI 线程之间共享，所以用 volatile / synchronized 兜着） */
    static final class Task {
        final String id = UUID.randomUUID().toString();
        final int direction;
        final long startedAt = System.currentTimeMillis();

        String serverId = "";
        String serverName = "";
        String name = "";          // 文件名
        String remoteDir = "";     // 远端所在目录（面包屑用）
        String remotePath = "";    // 远端完整路径
        String localPath = "";     // 下载落盘路径 / 上传来源显示名
        String message = "";       // 失败原因（失败时）/ 补充说明
        Uri sourceUri;             // 上传来源（重试 / 续传用；跨进程可能失效，失效了会明说）

        volatile long total = -1;  // 总字节，-1 = 未知（无法显示百分比，走不确定态）
        volatile long transferred;
        volatile long baseline;    // 本次启动时的起点（续传不为 0）—— 算速度要减掉，否则一续传就虚报
        volatile int state = STATE_RUNNING;
        volatile long finishedAt;
        volatile int stop = STOP_NONE;
        /** 这次是接着上次的断点传的（不是重头来） */
        volatile boolean resumed;

        Task(int direction) {
            this.direction = direction;
        }

        boolean isRunning() {
            return state == STATE_RUNNING;
        }

        /** 还挂在清单上等着人操作（进行中 / 已暂停）—— 这两态都能点按钮 */
        boolean isPending() {
            return state == STATE_RUNNING || state == STATE_PAUSED;
        }

        boolean shouldStop() {
            return stop != STOP_NONE;
        }

        boolean stoppingForPause() {
            return stop == STOP_PAUSE;
        }

        /** 0..1；总量未知时返回 0（UI 那边按不确定态画） */
        float fraction() {
            if (total <= 0) {
                return 0f;
            }
            float value = (float) ((double) transferred / (double) total);
            if (value < 0f) {
                return 0f;
            }
            return value > 1f ? 1f : value;
        }

        /** 平均速度（字节/秒）：只算**本次这一段**，续传带进来的那截不计 */
        long bytesPerSecond() {
            long span = (finishedAt > 0 ? finishedAt : System.currentTimeMillis()) - startedAt;
            if (span <= 0 || transferred <= baseline) {
                return 0L;
            }
            return (transferred - baseline) * 1000L / span;
        }
    }

    private static final Map<String, Task> TASKS = new LinkedHashMap<>();
    private static Observer observer;

    private TransferManager() {
    }

    static void setObserver(Observer callback) {
        observer = callback;
    }

    // ------------------------------------------------------------------
    // 查询
    // ------------------------------------------------------------------

    static List<Task> all() {
        synchronized (TASKS) {
            return new ArrayList<>(TASKS.values());
        }
    }

    /** 还在清单上的（进行中 + 已暂停；新的在前） */
    static List<Task> active() {
        List<Task> list = new ArrayList<>();
        for (Task task : all()) {
            if (task.isPending()) {
                list.add(0, task);
            }
        }
        return list;
    }

    /** 已收尾的（新的在前） */
    static List<Task> finished() {
        List<Task> list = new ArrayList<>();
        List<Task> snapshot = all();
        for (int i = snapshot.size() - 1; i >= 0; i--) {
            Task task = snapshot.get(i);
            if (!task.isPending()) {
                list.add(task);
            }
        }
        return list;
    }

    /** 最近一条传完的上传（文件页用它判断要不要重拉目录） */
    static Task newestFinishedUpload() {
        List<Task> snapshot = all();
        for (int i = snapshot.size() - 1; i >= 0; i--) {
            Task task = snapshot.get(i);
            if (task.direction == DIR_UPLOAD && !task.isPending()) {
                return task;
            }
        }
        return null;
    }

    static int runningCount() {
        int count = 0;
        for (Task task : all()) {
            if (task.isRunning()) {
                count++;
            }
        }
        return count;
    }

    /**
     * 同一方向、同一个远端路径上还挂着的任务（空 = 没有）。
     *
     * <p>用它挡住重复发起：两条任务同时写同一个文件（下载各自以覆盖方式打开本地文件、
     * 上传各自 APPEND 远端）会互相踩，出来的文件是坏的。连点两下按钮就能撞上。
     */
    static Task pendingFor(int direction, String remotePath) {
        for (Task task : all()) {
            if (task.isPending() && task.direction == direction
                    && task.remotePath != null && task.remotePath.equals(remotePath)) {
                return task;
            }
        }
        return null;
    }

    // ------------------------------------------------------------------
    // 发起 / 暂停 / 继续 / 取消 / 重试
    // ------------------------------------------------------------------

    /**
     * 上传：把本地（content Uri）的文件推到服务器目录里。
     *
     * @param prompt TOFU 指纹确认（走 UI 的确认框）；连接已建立时用不到
     */
    static Task startUpload(Context context, JumpServer server, String remoteDir,
                            Uri source, String fileName, SshSessionManager.Prompt prompt) {
        Task task = new Task(DIR_UPLOAD);
        task.serverId = server.id;
        task.serverName = labelOf(server);
        task.name = fileName;
        task.remoteDir = remoteDir;
        task.remotePath = joinRemote(remoteDir, fileName);
        task.sourceUri = source;
        task.localPath = fileName;
        task.total = sizeOf(context, source);
        register(task);
        final Context app = context.getApplicationContext();
        new Thread(() -> runUpload(app, task, server, prompt, false), "anyboot-xfer-up").start();
        return task;
    }

    /** 下载：把服务器上的文件拉到 App 的下载目录 */
    static Task startDownload(Context context, JumpServer server, String remoteDir,
                              String fileName, SshSessionManager.Prompt prompt) {
        Task task = new Task(DIR_DOWNLOAD);
        task.serverId = server.id;
        task.serverName = labelOf(server);
        task.name = fileName;
        task.remoteDir = remoteDir;
        task.remotePath = joinRemote(remoteDir, fileName);
        register(task);
        final Context app = context.getApplicationContext();
        new Thread(() -> runDownload(app, task, server, prompt, false), "anyboot-xfer-down").start();
        return task;
    }

    /**
     * 暂停：停下来但**保留半截文件**，之后「继续」从断点接着传。
     *
     * <p>实现上是让进度回调下一次返回 false（跟取消同一条机制），差别只在收尾时不清文件。
     */
    static void pause(String taskId) {
        Task task = find(taskId);
        if (task == null || !task.isRunning()) {
            return;
        }
        task.stop = STOP_PAUSE;
        notifyChanged();
    }

    /**
     * 继续：接着上次的断点传（**新建一条任务**，暂停那条记录留在原地当历史）。
     *
     * @return null = 已发起；否则是接不上的原因
     */
    static String resume(Context context, Task previous, SshSessionManager.Prompt prompt) {
        if (previous == null) {
            return "这条任务不在了";
        }
        if (previous.state != STATE_PAUSED) {
            return "这条任务不是暂停状态";
        }
        JumpServer server = JumpStore.findJump(JumpStore.loadJumps(context), previous.serverId);
        if (server == null) {
            return "服务器已删除，接不上了";
        }
        if (previous.direction == DIR_UPLOAD && previous.sourceUri == null) {
            return "找不到原来的本地文件，只能重新上传";
        }
        Task clashing = pendingFor(previous.direction, previous.remotePath);
        if (clashing != null && clashing != previous) {
            return "这条已经在传了";
        }

        Task task = new Task(previous.direction);
        task.serverId = previous.serverId;
        task.serverName = previous.serverName;
        task.name = previous.name;
        task.remoteDir = previous.remoteDir;
        task.remotePath = previous.remotePath;
        task.localPath = previous.localPath;
        task.sourceUri = previous.sourceUri;
        task.total = previous.total;
        task.transferred = previous.transferred;
        task.baseline = previous.transferred;
        // 暂停那条就此收尾（只是留个说法），否则它会挂成一行没有意义的「已暂停」
        finish(previous, STATE_SUPERSEDED, "已接续（本次从 "
                + humanBytes(previous.transferred) + " 接着传）");
        register(task);

        final Context app = context.getApplicationContext();
        final boolean upload = previous.direction == DIR_UPLOAD;
        new Thread(() -> {
            if (upload) {
                runUpload(app, task, server, prompt, true);
            } else {
                runDownload(app, task, server, prompt, true);
            }
        }, upload ? "anyboot-xfer-up" : "anyboot-xfer-down").start();
        return null;
    }

    /** 取消：让进度回调下一次返回 false，jsch 就地收尾（不断共享会话），并把半截文件清掉 */
    static void cancel(Context context, String taskId) {
        Task task = find(taskId);
        if (task == null || !task.isPending()) {
            return;
        }
        task.stop = STOP_CANCEL;
        if (task.state == STATE_PAUSED) {
            // 已经停下来了，没有传输线程会来收尾 —— 这里自己把半截文件处理掉
            boolean cleaned = discardPartial(context.getApplicationContext(), task);
            finish(task, STATE_CANCELLED, cancelledMessage(task, verbOf(task),
                    task.direction == DIR_DOWNLOAD
                            ? (cleaned ? "半截文件已删除" : "半截文件没能删掉")
                            : (cleaned ? "半截文件已清理" : "半截文件可能还在服务器上")));
            return;
        }
        notifyChanged();
    }

    /**
     * 重试：按原样再发一次（**新建一条任务**，原来的记录留着，看得见历史）。
     * 注意重试是**从头来**；要接着断点走得用 {@link #resume}。
     *
     * @return null = 已发起；否则是发不出去的原因
     */
    static String retry(Context context, Task previous, SshSessionManager.Prompt prompt) {
        if (previous == null || previous.isPending()) {
            return "这条任务还挂着";
        }
        JumpServer server = JumpStore.findJump(JumpStore.loadJumps(context), previous.serverId);
        if (server == null) {
            return "服务器已删除，重试不了";
        }
        if (pendingFor(previous.direction, previous.remotePath) != null) {
            return "这条已经在传了";
        }
        if (previous.direction == DIR_UPLOAD) {
            if (previous.sourceUri == null) {
                return "找不到原来的本地文件";
            }
            startUpload(context, server, previous.remoteDir, previous.sourceUri, previous.name, prompt);
            return null;
        }
        startDownload(context, server, previous.remoteDir, previous.name, prompt);
        return null;
    }

    /** 清掉所有已收尾的记录（进行中 / 暂停的都不动） */
    static void clearFinished() {
        synchronized (TASKS) {
            List<String> remove = new ArrayList<>();
            for (Task task : TASKS.values()) {
                if (!task.isPending()) {
                    remove.add(task.id);
                }
            }
            for (String id : remove) {
                TASKS.remove(id);
            }
        }
        notifyChanged();
    }

    // ------------------------------------------------------------------
    // 干活
    // ------------------------------------------------------------------

    private static void runUpload(Context app, Task task, JumpServer server,
                                  SshSessionManager.Prompt prompt, boolean resume) {
        ChannelSftp sftp = null;
        InputStream input = null;
        try {
            input = app.getContentResolver().openInputStream(task.sourceUri);
            if (input == null) {
                throw new Exception("打不开本地文件");
            }
            Session session = SshSessionManager.shared().ensureSession(app, server, prompt);
            sftp = openSftp(session);
            // 远端已经有多少 = 本地要跳过多少（jsch 的 RESUME 自己会 skip 源流，这里只是给进度条定起点）
            long startAt = resume ? Math.max(sizeAt(sftp, task.remotePath), 0L) : 0L;
            task.resumed = resume && startAt > 0;
            task.transferred = startAt;
            if (resume) {
                sftp.put(input, task.remotePath, new Progress(task, startAt, false), ChannelSftp.RESUME);
            } else {
                sftp.put(input, task.remotePath, new Progress(task, 0, false));
            }
            if (task.shouldStop()) {
                stopUpload(sftp, task);
                return;
            }
            settleAtTotal(task);
            finish(task, STATE_DONE, "已上传到 " + task.remotePath);
        } catch (Exception error) {
            if (task.shouldStop()) {
                stopUpload(sftp, task);
                return;
            }
            finish(task, STATE_FAILED, reason(error, "上传失败"));
        } finally {
            closeQuietly(input);
            close(sftp);
        }
    }

    private static void stopUpload(ChannelSftp sftp, Task task) {
        if (task.stoppingForPause()) {
            // 远端那半截留着 —— 它就是「继续」的起点
            finish(task, STATE_PAUSED, pausedMessage(task, "已传"));
            return;
        }
        cleanupRemote(sftp, task);
        finish(task, STATE_CANCELLED, cancelledMessage(task, "已传", "半截文件已清理"));
    }

    private static void runDownload(Context app, Task task, JumpServer server,
                                    SshSessionManager.Prompt prompt, boolean resume) {
        ChannelSftp sftp = null;
        File target = null;
        java.io.OutputStream out = null;
        try {
            Session session = SshSessionManager.shared().ensureSession(app, server, prompt);
            sftp = openSftp(session);
            task.total = sizeAt(sftp, task.remotePath);

            // 续传认原来那个半截文件；否则（首传 / 半截文件没了）新开一个
            target = (resume && !task.localPath.isEmpty())
                    ? new File(task.localPath)
                    : newTarget(app, task);

            long already = (resume && target.isFile()) ? target.length() : 0L;
            if (already > 0 && task.total > 0 && already > task.total) {
                // 远端被换小了，断点对不上 —— 留着只会拼出一个坏文件，删掉重下
                TerminalActivity.tlog("传输：断点 " + already + " 大于远端 " + task.total + "，改为重下");
                cleanupLocal(target);
                already = 0L;
            }

            if (resume && already > 0) {
                task.resumed = true;
                task.transferred = already;
                // RESUME 变体：jsch 自己 _stat 远端、比对本地长度，从断点 READ；
                // 它还会紧跟 init 补一次 count(已下) —— 那一笔由 swallowFirstCount 当基线吃掉
                sftp.get(task.remotePath, target.getAbsolutePath(),
                        new Progress(task, 0, true), ChannelSftp.RESUME);
            } else {
                out = new java.io.FileOutputStream(target);
                sftp.get(task.remotePath, out, new Progress(task, 0, false));
            }
            if (task.shouldStop()) {
                stopDownload(target, task);
                return;
            }
            settleAtTotal(task);
            finish(task, STATE_DONE, "已下载到 " + task.localPath);
        } catch (Exception error) {
            if (task.shouldStop()) {
                stopDownload(target, task);
                return;
            }
            finish(task, STATE_FAILED, reason(error, "下载失败"));
        } finally {
            closeQuietly(out);
            close(sftp);
        }
    }

    private static void stopDownload(File target, Task task) {
        if (task.stoppingForPause()) {
            // 本地那半截留着 —— 它就是「继续」的起点
            finish(task, STATE_PAUSED, pausedMessage(task, "已下"));
            return;
        }
        cleanupLocal(target);
        finish(task, STATE_CANCELLED, cancelledMessage(task, "已下", "半截文件已删除"));
    }

    /** 正常传完时把进度钉到 100%：RESUME 变体在「本地已跟远端一样大」时会直接返回、一个 count 都不报 */
    private static void settleAtTotal(Task task) {
        if (task.total > 0) {
            task.transferred = task.total;
        }
    }

    private static File newTarget(Context app, Task task) {
        File dir = app.getExternalFilesDir(android.os.Environment.DIRECTORY_DOWNLOADS);
        if (dir == null) {
            dir = app.getFilesDir();
        }
        dir.mkdirs();
        File target = new File(dir, task.name);
        task.localPath = target.getAbsolutePath();
        return target;
    }

    /**
     * 取消一条**已经暂停**的任务时清掉半截文件（下载删本地，上传借共享会话删远端）。
     *
     * @return 是否确认清掉了
     */
    private static boolean discardPartial(Context app, Task task) {
        if (task.direction == DIR_DOWNLOAD) {
            if (task.localPath.isEmpty()) {
                return true;
            }
            File file = new File(task.localPath);
            if (!file.exists()) {
                return true;
            }
            cleanupLocal(file);
            return !file.exists();
        }
        ChannelSftp sftp = null;
        try {
            Session session = SshSessionManager.shared().liveSession(task.serverId);
            if (session == null) {
                return false;   // 会话已经被回收，远端那半截这次清不掉
            }
            sftp = openSftp(session);
            sftp.rm(task.remotePath);
            return true;
        } catch (Exception ignored) {
            return false;
        } finally {
            close(sftp);
        }
    }

    private static ChannelSftp openSftp(Session session) throws Exception {
        // 与文件页同款：共享会话上另开一条 sftp 通道
        ChannelSftp sftp = (ChannelSftp) session.openChannel("sftp");
        sftp.connect(15000);
        return sftp;
    }

    private static void closeQuietly(java.io.Closeable stream) {
        if (stream == null) {
            return;
        }
        try {
            stream.close();
        } catch (Exception ignored) {
            // 关不上也无所谓，进程会收
        }
    }

    private static void close(ChannelSftp sftp) {
        if (sftp == null) {
            return;
        }
        try {
            sftp.disconnect();
        } catch (Exception ignored) {
            // 收尾失败不影响任务结论
        }
    }

    /** 取消上传后尽力清掉远端那半截文件（清不掉也只是留个残文件，不影响结论） */
    private static void cleanupRemote(ChannelSftp sftp, Task task) {
        if (sftp == null || task.transferred <= 0) {
            return;
        }
        try {
            sftp.rm(task.remotePath);
        } catch (Exception ignored) {
            // 远端只读 / 文件已被宿主收走
        }
    }

    private static void cleanupLocal(File target) {
        if (target != null && target.exists() && !target.delete()) {
            TerminalActivity.tlog("传输：半截文件删不掉 " + target.getAbsolutePath());
        }
    }

    private static void register(Task task) {
        synchronized (TASKS) {
            TASKS.put(task.id, task);
            // 收尾的超过上限就把最老的丢掉（LinkedHashMap 保持插入序）
            List<String> finishedIds = new ArrayList<>();
            for (Task item : TASKS.values()) {
                if (!item.isPending()) {
                    finishedIds.add(item.id);
                }
            }
            for (int i = 0; i < finishedIds.size() - FINISHED_LIMIT; i++) {
                TASKS.remove(finishedIds.get(i));
            }
        }
        notifyChanged();
    }

    private static void finish(Task task, int state, String message) {
        task.state = state;
        task.message = message;
        task.finishedAt = System.currentTimeMillis();
        // 收尾都写一行 logcat（这类问题光看界面看不出方向）
        TerminalActivity.tlog("传输 " + (task.direction == DIR_UPLOAD ? "上传" : "下载")
                + " " + task.name + " -> " + state + " " + message);
        notifyChanged();
    }

    private static Task find(String taskId) {
        synchronized (TASKS) {
            return TASKS.get(taskId);
        }
    }

    private static void notifyChanged() {
        Observer callback = observer;
        if (callback != null) {
            callback.onTransfersChanged();
        }
    }

    // ------------------------------------------------------------------
    // 进度回调
    // ------------------------------------------------------------------

    /**
     * jsch 的进度回调：count 给的是本块字节数（增量），返回 false = 原地停下来。
     *
     * @param initialTransferred  本次启动时的起点（上传续传时是远端已传长度；其余传 0）
     * @param swallowFirstCount   是否把第一次 count 当基线吃掉。下载续传时 jsch 会在 init 后
     *                            补一次 {@code count(已下字节)}，那一笔是起点不是进度 ——
     *                            不特判的话速度会从 0 开始虚报成「几十 MB/s」
     */
    private static final class Progress implements SftpProgressMonitor {

        private final Task task;
        private final long initialTransferred;
        private boolean swallowFirst;
        private long lastNotify;

        Progress(Task task, long initialTransferred, boolean swallowFirstCount) {
            this.task = task;
            this.initialTransferred = initialTransferred;
            this.swallowFirst = swallowFirstCount;
        }

        @Override
        public void init(int op, String src, String dest, long max) {
            if (max > 0) {
                task.total = max;
            }
            task.transferred = initialTransferred;
            task.baseline = initialTransferred;
            notifyChanged();
        }

        @Override
        public boolean count(long count) {
            if (swallowFirst) {
                swallowFirst = false;
                task.transferred += count;
                task.baseline = task.transferred;
                notifyChanged();
                return !task.shouldStop();
            }
            task.transferred += count;
            long now = System.currentTimeMillis();
            // 4Hz 上限：SFTP 的 count 回调非常密，每块都刷会把主线程压住。
            // 传完那一刻（transferred 追平 total）必须刷一次，否则进度条会停在 99%。
            boolean reachedEnd = task.total > 0 && task.transferred >= task.total;
            if (now - lastNotify >= 250 || reachedEnd) {
                lastNotify = now;
                notifyChanged();
            }
            return !task.shouldStop();
        }

        @Override
        public void end() {
            notifyChanged();
        }
    }

    // ------------------------------------------------------------------
    // 小件
    // ------------------------------------------------------------------

    static String labelOf(JumpServer server) {
        String name = JumpStore.text(server.name);
        return name.isEmpty() ? server.addressLabel() : name;
    }

    private static String verbOf(Task task) {
        return task.direction == DIR_UPLOAD ? "已传" : "已下";
    }

    private static String pausedMessage(Task task, String verb) {
        String size = task.transferred > 0 ? "（" + verb + " " + humanBytes(task.transferred) + "）" : "";
        return "已暂停" + size + "，点「继续」接着传";
    }

    private static String cancelledMessage(Task task, String verb, String note) {
        return task.transferred > 0
                ? "已取消（" + verb + " " + humanBytes(task.transferred) + "，" + note + "）"
                : "已取消";
    }

    /** 字节数给人看（与 Common.kt 的 humanSize 同一个口径；Java 这边调不动 Kotlin 顶层函数，各留一份） */
    static String humanBytes(long bytes) {
        if (bytes < 1024) {
            return bytes + " B";
        }
        if (bytes < 1024L * 1024L) {
            return String.format(java.util.Locale.US, "%.1f KB", bytes / 1024.0);
        }
        return String.format(java.util.Locale.US, "%.1f MB", bytes / 1024.0 / 1024.0);
    }

    private static String joinRemote(String dir, String name) {
        if (dir == null || dir.isEmpty() || "/".equals(dir)) {
            return "/" + name;
        }
        return dir.endsWith("/") ? dir + name : dir + "/" + name;
    }

    /** content Uri 的总字节数（拿不到就 -1：进度条走不确定态，不假装知道） */
    private static long sizeOf(Context context, Uri uri) {
        if (uri == null) {
            return -1;
        }
        try (AssetFileDescriptor fd = context.getContentResolver().openAssetFileDescriptor(uri, "r")) {
            if (fd != null && fd.getLength() > 0) {
                return fd.getLength();
            }
        } catch (Exception ignored) {
            // 有些 provider 不给长度
        }
        return -1;
    }

    private static long sizeAt(ChannelSftp sftp, String path) {
        try {
            return sftp.lstat(path).getSize();
        } catch (Exception ignored) {
            return -1;
        }
    }

    private static String reason(Exception error, String fallback) {
        String message = error.getMessage();
        return fallback + "：" + (message == null || message.isEmpty()
                ? error.getClass().getSimpleName() : message);
    }
}
