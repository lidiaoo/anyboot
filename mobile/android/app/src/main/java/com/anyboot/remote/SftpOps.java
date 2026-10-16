package com.anyboot.remote;

import android.content.Context;

import com.jcraft.jsch.ChannelSftp;
import com.jcraft.jsch.Session;
import com.jcraft.jsch.SftpATTRS;
import com.jcraft.jsch.SftpException;

import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.List;
import java.util.Vector;

/**
 * SFTP 文件操作（列表 / 详情 / 新建目录 / 改名 / 删除 / 复制 / 移动 / 解析路径）。
 *
 * <p>与 {@link TransferManager} 的分工：那边管「带进度、能暂停、要落盘」的文件内容搬运；
 * 这边管「一会儿就完事」的目录级动作。两者复用 {@link SshSessionManager} 的同一条 SSH 会话，
 * 各自开一条短命的 sftp 通道，用完就关（互不干扰，也不会把交互式终端踢下线）。
 *
 * <p><b>所有方法都是阻塞的，必须在后台线程调用</b>，异常信息已经是能直接给用户看的中文。
 */
final class SftpOps {

    private SftpOps() {
    }

    /** 一次「开通道 → 干活 → 关通道」的动作体 */
    private interface Work<T> {
        T run(ChannelSftp sftp) throws Exception;
    }

    /** 需要同时开两条通道（一条只读、一条只写）的动作体 —— 复制和跨设备移动用 */
    private interface TwoWork<T> {
        T run(ChannelSftp reader, ChannelSftp writer) throws Exception;
    }

    // ------------------------------------------------------------------
    // 数据结构
    // ------------------------------------------------------------------

    /** 目录里的一项（只带列表要用的字段，够排序和展示） */
    static final class Entry {

        final String name;
        final boolean dir;
        final boolean link;
        final long size;
        /** 修改时间，秒（SFTP 原始单位，展示时再 ×1000） */
        final long mtime;
        /** 形如 rwxr-xr-x（不含最前面的类型字符） */
        final String permissions;

        Entry(String name, boolean dir, boolean link, long size, long mtime, String permissions) {
            this.name = name;
            this.dir = dir;
            this.link = link;
            this.size = size;
            this.mtime = mtime;
            this.permissions = permissions == null ? "" : permissions;
        }

        /** 列表行最左边那个图标 —— 软链接单独一个，免得看着像目录其实指向别处 */
        String icon() {
            if (link) {
                return "🔗";
            }
            return dir ? "📁" : "📄";
        }
    }

    /** 「文件详情」弹窗要的全部字段 */
    static final class Detail {
        String path = "";
        String name = "";
        String linkTarget = "";
        String permissions = "";
        String typeLabel = "";
        boolean dir;
        boolean link;
        long size;
        long mtime;
        long atime;
        int uid;
        int gid;
    }

    // ------------------------------------------------------------------
    // 通道
    // ------------------------------------------------------------------

    private static <T> T with(Context context, JumpServer server, SshSessionManager.Prompt prompt,
                              Work<T> work) throws Exception {
        Session session = SshSessionManager.shared().ensureSession(context, server, prompt);
        ChannelSftp sftp = (ChannelSftp) session.openChannel("sftp");
        sftp.connect(15000);
        try {
            return work.run(sftp);
        } finally {
            close(sftp);
        }
    }

    /**
     * 开两条 sftp 通道：reader 只读、writer 只写。
     *
     * <p><b>为什么非要两条</b>：jsch 的 ChannelSftp 在同一条通道上同时开读流和写流会死锁 ——
     * 写完一个数据包之后 {@code read()} 再也不返回（详见 {@link #copyTree} 的注释）。
     * 两条通道各自有独立的请求队列和读线程，互不干扰。
     */
    private static <T> T withTwo(Context context, JumpServer server, SshSessionManager.Prompt prompt,
                                 TwoWork<T> work) throws Exception {
        Session session = SshSessionManager.shared().ensureSession(context, server, prompt);
        ChannelSftp reader = (ChannelSftp) session.openChannel("sftp");
        reader.connect(15000);
        try {
            ChannelSftp writer = (ChannelSftp) session.openChannel("sftp");
            writer.connect(15000);
            try {
                return work.run(reader, writer);
            } finally {
                close(writer);
            }
        } finally {
            close(reader);
        }
    }

    private static void close(ChannelSftp channel) {
        try {
            channel.disconnect();
        } catch (Exception ignored) {
            // 关不掉就算了，别把真正的失败原因盖掉
        }
    }

    // ------------------------------------------------------------------
    // 路径工具（和 TransferManager.joinRemote 一个口径）
    // ------------------------------------------------------------------

    static String join(String dir, String name) {
        if (name == null || name.isEmpty()) {
            return dir == null ? "" : dir;
        }
        if (name.startsWith("/")) {
            return name;
        }
        if (dir == null || dir.isEmpty() || "/".equals(dir)) {
            return "/" + name;
        }
        return dir.endsWith("/") ? dir + name : dir + "/" + name;
    }

    static String parent(String path) {
        if (path == null || path.isEmpty() || "/".equals(path)) {
            return "/";
        }
        String trimmed = path.endsWith("/") ? path.substring(0, path.length() - 1) : path;
        int cut = trimmed.lastIndexOf('/');
        return cut <= 0 ? "/" : trimmed.substring(0, cut);
    }

    static String nameOf(String path) {
        if (path == null || path.isEmpty()) {
            return "";
        }
        String trimmed = path.endsWith("/") ? path.substring(0, path.length() - 1) : path;
        int cut = trimmed.lastIndexOf('/');
        return cut < 0 ? trimmed : trimmed.substring(cut + 1);
    }

    /** 给 shell 用的单引号转义（终端里 cd 过去时用） */
    static String shellQuote(String path) {
        return "'" + (path == null ? "" : path.replace("'", "'\\''")) + "'";
    }

    // ------------------------------------------------------------------
    // 读
    // ------------------------------------------------------------------

    /** 列目录（不含 . 和 ..） */
    static List<Entry> list(Context context, JumpServer server, String path,
                            SshSessionManager.Prompt prompt) throws Exception {
        return with(context, server, prompt, sftp -> {
            List<Entry> out = new ArrayList<>();
            Vector<ChannelSftp.LsEntry> raw = sftp.ls(path);
            for (ChannelSftp.LsEntry item : raw) {
                String name = item.getFilename();
                if (".".equals(name) || "..".equals(name)) {
                    continue;
                }
                // 个别服务器回目录名带尾斜杠；尾斜杠还会把排序和路径拼接一起带错
                while (name.length() > 1 && name.endsWith("/")) {
                    name = name.substring(0, name.length() - 1);
                }
                SftpATTRS attrs = item.getAttrs();
                out.add(new Entry(name, attrs.isDir(), attrs.isLink(),
                        attrs.getSize(), attrs.getMTime(), attrs.getPermissionsString()));
            }
            return out;
        });
    }

    /** 取一项的详细信息（软链接会连指向目标一起给） */
    static Detail detail(Context context, JumpServer server, String path,
                         SshSessionManager.Prompt prompt) throws Exception {
        return with(context, server, prompt, sftp -> {
            Detail detail = new Detail();
            detail.path = path;
            detail.name = nameOf(path);
            SftpATTRS attrs = sftp.lstat(path);
            fill(detail, attrs);
            if (attrs.isLink()) {
                try {
                    detail.linkTarget = sftp.readlink(path);
                } catch (Exception ignored) {
                    // 有些服务器不给读 link，留空即可
                }
            }
            return detail;
        });
    }

    private static void fill(Detail detail, SftpATTRS attrs) {
        detail.size = attrs.getSize();
        detail.mtime = attrs.getMTime();
        detail.atime = attrs.getATime();
        detail.uid = attrs.getUId();
        detail.gid = attrs.getGId();
        detail.permissions = attrs.getPermissionsString();
        detail.dir = attrs.isDir();
        detail.link = attrs.isLink();
        if (detail.link) {
            detail.typeLabel = "软链接";
        } else if (attrs.isDir()) {
            detail.typeLabel = "目录";
        } else {
            detail.typeLabel = "文件";
        }
    }

    /** 把路径解析成服务器上的规范绝对路径；不存在 / 没权限会抛异常（正好当「路径合法吗」的校验） */
    static String resolve(Context context, JumpServer server, String path,
                          SshSessionManager.Prompt prompt) throws Exception {
        return with(context, server, prompt, sftp -> sftp.realpath(path));
    }

    /** 登录用户的家目录 */
    static String home(Context context, JumpServer server, SshSessionManager.Prompt prompt)
            throws Exception {
        return with(context, server, prompt, sftp -> sftp.getHome());
    }

    // ------------------------------------------------------------------
    // 写
    // ------------------------------------------------------------------

    static void mkdir(Context context, JumpServer server, String path,
                      SshSessionManager.Prompt prompt) throws Exception {
        with(context, server, prompt, sftp -> {
            sftp.mkdir(path);
            return null;
        });
    }

    /** 改名 / 移动（同一分区下改名即可，跨设备由调用方走 {@link #move}） */
    static void rename(Context context, JumpServer server, String from, String to,
                       SshSessionManager.Prompt prompt) throws Exception {
        with(context, server, prompt, sftp -> {
            sftp.rename(from, to);
            return null;
        });
    }

    /** 删除：目录递归删干净（SFTP 没有 rm -r，只能自己走） */
    static void delete(Context context, JumpServer server, String path,
                       SshSessionManager.Prompt prompt) throws Exception {
        with(context, server, prompt, sftp -> {
            removeTree(sftp, path);
            return null;
        });
    }

    /** 复制（目录递归；目标已存在同名项时由调用方先改名，这里不做覆盖判断） */
    static void copy(Context context, JumpServer server, String from, String to,
                     SshSessionManager.Prompt prompt) throws Exception {
        withTwo(context, server, prompt, (reader, writer) -> {
            copyTree(reader, writer, from, to);
            return null;
        });
    }

    /**
     * 移动：先试 rename（同一分区瞬时完成），失败再退化成「复制 + 删源」。
     * 跨分区时 SFTP 的 rename 会返回失败，这是唯一能走的办法。
     */
    static void move(Context context, JumpServer server, String from, String to,
                     SshSessionManager.Prompt prompt) throws Exception {
        withTwo(context, server, prompt, (reader, writer) -> {
            try {
                reader.rename(from, to);
                return null;
            } catch (SftpException ignored) {
                TerminalActivity.tlog("文件：rename 失败，退化成复制+删除 " + from + " -> " + to);
            }
            copyTree(reader, writer, from, to);
            removeTree(reader, from);
            return null;
        });
    }

    /** 目标不存在才返回 true（用来决定要不要加「(2)」后缀） */
    static boolean exists(Context context, JumpServer server, String path,
                          SshSessionManager.Prompt prompt) throws Exception {
        return with(context, server, prompt, sftp -> {
            try {
                sftp.lstat(path);
                return Boolean.TRUE;
            } catch (SftpException e) {
                return Boolean.FALSE;
            }
        });
    }

    // ------------------------------------------------------------------
    // 递归
    // ------------------------------------------------------------------

    private static void removeTree(ChannelSftp sftp, String path) throws Exception {
        SftpATTRS attrs = sftp.lstat(path);
        // 软链接当文件删 —— 删链接本身，别把指向的目标也拆了
        if (attrs.isDir() && !attrs.isLink()) {
            Vector<ChannelSftp.LsEntry> raw = sftp.ls(path);
            for (ChannelSftp.LsEntry item : raw) {
                String name = item.getFilename();
                if (".".equals(name) || "..".equals(name)) {
                    continue;
                }
                removeTree(sftp, join(path, name));
            }
            sftp.rmdir(path);
        } else {
            sftp.rm(path);
        }
    }

    /**
     * 递归复制。读走 {@code reader}、写走 {@code writer}，<b>两条通道必须是不同的对象</b>。
     *
     * <p><b>⚠️ 别把两者合回一条通道。</b> jsch 的 ChannelSftp 在一条通道上同时开
     * {@code get()} 的读流和 {@code put()} 的写流会死锁：只写出一个数据包（2 MiB 的源
     * 只落 32755 字节），之后 {@code read()} 永远不返回，整个操作挂死。
     * 实机（App 的「复制到…」）与 JVM 对照（{@code ~/.cache/ab-droid/SftpCopyProbe.java}
     * 的 A/C 两组）都复现过。分两条通道直连对流：2 MiB 文件 5.3 s，
     * 比「先落本地临时文件再上传」的 10.4 s 快一倍，还不占本地磁盘。
     */
    private static void copyTree(ChannelSftp reader, ChannelSftp writer, String from, String to)
            throws Exception {
        SftpATTRS attrs = reader.lstat(from);
        if (attrs.isDir() && !attrs.isLink()) {
            try {
                writer.mkdir(to);
            } catch (SftpException ignored) {
                // 已经存在就当目录可用，继续往里拷
            }
            Vector<ChannelSftp.LsEntry> raw = reader.ls(from);
            for (ChannelSftp.LsEntry item : raw) {
                String name = item.getFilename();
                if (".".equals(name) || "..".equals(name)) {
                    continue;
                }
                copyTree(reader, writer, join(from, name), join(to, name));
            }
            return;
        }
        InputStream in = reader.get(from);
        try {
            OutputStream out = writer.put(to);
            try {
                byte[] buffer = new byte[64 * 1024];
                int read;
                while ((read = in.read(buffer)) > 0) {
                    out.write(buffer, 0, read);
                }
            } finally {
                out.close();
            }
        } finally {
            in.close();
        }
    }

    // ------------------------------------------------------------------
    // 出错信息
    // ------------------------------------------------------------------

    /**
     * 把 jsch 的英文失败原因换成能看懂的中文。
     *
     * <p>优先看协议错误码（{@link SftpException#id}）—— 那个是服务器给的、
     * 不受语言和措辞影响；认不出来才退回字符串匹配，最后才把原文带上。
     */
    static String reason(Exception error, String action, String path) {
        if (error instanceof SftpException) {
            int id = ((SftpException) error).id;
            if (id == ChannelSftp.SSH_FX_NO_SUCH_FILE) {
                return action + "失败：服务器上找不到「" + path + "」";
            }
            if (id == ChannelSftp.SSH_FX_PERMISSION_DENIED) {
                return action + "失败：权限不够（" + path + "）";
            }
            if (id == ChannelSftp.SSH_FX_OP_UNSUPPORTED) {
                return action + "失败：这台服务器不支持这个操作";
            }
            if (id == ChannelSftp.SSH_FX_CONNECTION_LOST
                    || id == ChannelSftp.SSH_FX_NO_CONNECTION) {
                return action + "失败：连接断了，重进这一页会自动重连";
            }
        }
        String raw = error.getMessage() == null ? "" : error.getMessage();
        String lower = raw.toLowerCase(java.util.Locale.ROOT);
        if (lower.contains("no such file")) {
            return action + "失败：服务器上找不到「" + path + "」";
        }
        if (lower.contains("permission denied")) {
            return action + "失败：权限不够（" + path + "）";
        }
        if (lower.contains("failure")) {
            return action + "失败：服务器拒绝了（" + path + "）";
        }
        return action + "失败：" + (raw.isEmpty() ? error.getClass().getSimpleName() : raw);
    }
}
