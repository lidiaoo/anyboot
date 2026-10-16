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
 * <p>「文件页」只负责发起，进度、取消、重试、结果都记在这里，「传输页」展示这份清单。
 * 传输复用 {@link SshSessionManager} 那条共享 SSH 会话上另开的 sftp 通道 —— 与终端、
 * 隧道互不影响。
 *
 * <p>取消的实现依据（已用 {@code javap -c} 核过 jsch 0.2.20 的 ChannelSftp 字节码）：
 * {@link SftpProgressMonitor#count(long)} 收到的是**本块新增字节数**（不是累计值），
 * 返回 {@code false} 时 put/get 的就地循环直接 break，剩余数据被 skip 掉后正常收尾 ——
 * 所以取消既不会抛异常，也不用去断共享会话。代价是远端/本地会留半截文件，
 * 这里尽力清理掉。
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
        Uri sourceUri;             // 上传来源（重试用；跨进程可能失效，失效了会明说）

        volatile long total = -1;  // 总字节，-1 = 未知（无法显示百分比，走不确定态）
        volatile long transferred;
        volatile int state = STATE_RUNNING;
        volatile long finishedAt;
        volatile boolean cancelRequested;

        Task(int direction) {
            this.direction = direction;
        }

        boolean isRunning() {
            return state == STATE_RUNNING;
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

        /** 平均速度（字节/秒），还没开始或已结束时按实际耗时算 */
        long bytesPerSecond() {
            long span = (finishedAt > 0 ? finishedAt : System.currentTimeMillis()) - startedAt;
            if (span <= 0) {
                return 0L;
            }
            return transferred * 1000L / span;
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

    /** 正在传的（新的在前） */
    static List<Task> active() {
        List<Task> list = new ArrayList<>();
        for (Task task : all()) {
            if (task.isRunning()) {
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
            if (!task.isRunning()) {
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
            if (task.direction == DIR_UPLOAD && !task.isRunning()) {
                return task;
            }
        }
        return null;
    }

    static int runningCount() {
        return active().size();
    }

    // ------------------------------------------------------------------
    // 发起 / 取消 / 重试
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
        new Thread(() -> runUpload(app, task, server, prompt), "anyboot-xfer-up").start();
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
        new Thread(() -> runDownload(app, task, server, prompt), "anyboot-xfer-down").start();
        return task;
    }

    /** 取消：让进度回调下一次返回 false，jsch 就地收尾（不断共享会话） */
    static void cancel(String taskId) {
        Task task = find(taskId);
        if (task == null || !task.isRunning()) {
            return;
        }
        task.cancelRequested = true;
        notifyChanged();
    }

    /**
     * 重试：按原样再发一次（**新建一条任务**，原来的失败记录留着，看得见历史）。
     *
     * @return null = 已发起；否则是发不出去的原因
     */
    static String retry(Context context, Task previous, SshSessionManager.Prompt prompt) {
        if (previous == null || previous.isRunning()) {
            return "这条任务还在传";
        }
        JumpServer server = JumpStore.findJump(JumpStore.loadJumps(context), previous.serverId);
        if (server == null) {
            return "服务器已删除，重试不了";
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

    /** 清掉所有已收尾的记录（正在传的不动） */
    static void clearFinished() {
        synchronized (TASKS) {
            List<String> remove = new ArrayList<>();
            for (Task task : TASKS.values()) {
                if (!task.isRunning()) {
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
                                  SshSessionManager.Prompt prompt) {
        ChannelSftp sftp = null;
        InputStream input = null;
        try {
            input = app.getContentResolver().openInputStream(task.sourceUri);
            if (input == null) {
                throw new Exception("打不开本地文件");
            }
            Session session = SshSessionManager.shared().ensureSession(app, server, prompt);
            sftp = openSftp(session);
            sftp.put(input, task.remotePath, new Progress(task));
            if (task.cancelRequested) {
                cleanupRemote(sftp, task);
                finish(task, STATE_CANCELLED, "已取消" + (task.transferred > 0
                        ? "（已传 " + humanBytes(task.transferred) + "，半截文件已清理）" : ""));
                return;
            }
            finish(task, STATE_DONE, "已上传到 " + task.remotePath);
        } catch (Exception error) {
            if (task.cancelRequested) {
                cleanupRemote(sftp, task);
                finish(task, STATE_CANCELLED, "已取消");
                return;
            }
            finish(task, STATE_FAILED, reason(error, "上传失败"));
        } finally {
            closeQuietly(input);
            close(sftp);
        }
    }

    private static void runDownload(Context app, Task task, JumpServer server,
                                    SshSessionManager.Prompt prompt) {
        ChannelSftp sftp = null;
        File target = null;
        java.io.OutputStream out = null;
        try {
            Session session = SshSessionManager.shared().ensureSession(app, server, prompt);
            sftp = openSftp(session);
            task.total = sizeAt(sftp, task.remotePath);
            File dir = app.getExternalFilesDir(android.os.Environment.DIRECTORY_DOWNLOADS);
            if (dir == null) {
                dir = app.getFilesDir();
            }
            dir.mkdirs();
            target = new File(dir, task.name);
            task.localPath = target.getAbsolutePath();
            out = new java.io.FileOutputStream(target);
            sftp.get(task.remotePath, out, new Progress(task));
            if (task.cancelRequested) {
                cleanupLocal(target);
                finish(task, STATE_CANCELLED, "已取消" + (task.transferred > 0
                        ? "（已下 " + humanBytes(task.transferred) + "，半截文件已删除）" : ""));
                return;
            }
            finish(task, STATE_DONE, "已下载到 " + task.localPath);
        } catch (Exception error) {
            if (task.cancelRequested) {
                cleanupLocal(target);
                finish(task, STATE_CANCELLED, "已取消");
                return;
            }
            finish(task, STATE_FAILED, reason(error, "下载失败"));
        } finally {
            closeQuietly(out);
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
                if (!item.isRunning()) {
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
        // 失败也把堆栈写进 logcat（跟密钥生成那处一个道理：这类失败光看界面看不出方向）
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

    /** jsch 的进度回调：count 给的是本块字节数（增量），返回 false = 中止 */
    private static final class Progress implements SftpProgressMonitor {

        private final Task task;
        private long lastNotify;

        Progress(Task task) {
            this.task = task;
        }

        @Override
        public void init(int op, String src, String dest, long max) {
            if (max > 0) {
                task.total = max;
            }
            task.transferred = 0;
            notifyChanged();
        }

        @Override
        public boolean count(long count) {
            task.transferred += count;
            long now = System.currentTimeMillis();
            // 4Hz 上限：SFTP 的 count 回调非常密，每块都刷会把主线程压住。
            // 传完那一刻（transferred 追平 total）必须刷一次，否则进度条会停在 99%。
            boolean reachedEnd = task.total > 0 && task.transferred >= task.total;
            if (now - lastNotify >= 250 || reachedEnd) {
                lastNotify = now;
                notifyChanged();
            }
            return !task.cancelRequested;
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
