package com.anyboot.remote;

import android.content.Context;
import android.util.Base64;

import com.jcraft.jsch.ChannelExec;
import com.jcraft.jsch.Session;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * 网络唤醒（Wake-on-LAN）：给一块网卡发「魔术包」，让关机/睡眠的电脑开机。
 *
 * <p>两条通道，按有没有配跳板机自动选：
 * <ul>
 *   <li><b>配了 SSH 跳板机</b>：登录跳板机，由它在内网广播 —— 手机在外网也能唤醒家里；
 *       优先 {@code python3}（现拼魔术包，绝大多数 Linux 都有），退而用
 *       {@code wakeonlan} / {@code etherwake}。</li>
 *   <li><b>没配跳板机</b>：手机自己往局域网 UDP 广播（端口 9 和 7），
 *       只有手机和这台电脑在同一网段时有效。</li>
 * </ul>
 *
 * <p>MAC 从 {@link ServerProfile#allMacs()} 取（地址行里跟在 IP 后面的那些）。
 * 一台电脑配了多块网卡的 MAC 时全部发一遍 —— 不知道现在插的是哪根网线。
 */
final class WakeOnLan {

    /** 魔术包端口：9 是标准，7 是老实现（有些网卡只认 7） */
    private static final int[] PORTS = {9, 7};

    /** 跳板机侧的 python 脚本：自己拼包，不依赖装了什么 WOL 工具 */
    private static final String PY_SCRIPT =
            "import socket,sys\n"
                    + "targets=[sys.argv[1],'255.255.255.255']\n"
                    + "macs=sys.argv[2:]\n"
                    + "s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)\n"
                    + "s.setsockopt(socket.SOL_SOCKET,socket.SO_BROADCAST,1)\n"
                    + "for m in macs:\n"
                    + "    h=m.replace(':','').replace('-','')\n"
                    + "    p=b'\\xff'*6+bytes.fromhex(h)*16\n"
                    + "    for t in targets:\n"
                    + "        for port in (9,7):\n"
                    + "            try: s.sendto(p,(t,port))\n"
                    + "            except Exception: pass\n"
                    + "s.close()\n";

    private WakeOnLan() {
    }

    /**
     * 发唤醒包。**必须在后台线程调用**（要连 SSH / 发 UDP）。
     *
     * @param prompt 跳板机首次连接时的主机指纹确认（TOFU），传 MainActivity 的确认框
     * @return 给用户看的提示
     * @throws Exception 没配 MAC / 跳板机不可用 / 发包失败
     */
    static String wake(Context context, ServerProfile profile, SshSessionManager.Prompt prompt)
            throws Exception {
        List<String> macs = profile.allMacs();
        if (macs.isEmpty()) {
            throw new Exception(
                    "这台电脑还没配 MAC —— 长按标签 → 在地址后面跟上网卡 MAC（如 192.168.0.120:12300 AA:BB:CC:DD:EE:FF）");
        }
        String jumpId = ServerProfile.text(profile.jumpServerId);
        if (!jumpId.isEmpty()) {
            return wakeViaJump(context, profile, jumpId, macs, prompt);
        }
        return wakeLocal(profile, macs);
    }

    // ------------------------------------------------------------------
    // 局域网电源路由（sol）
    // ------------------------------------------------------------------

    /** 一个待发的 UDP 包（times = 发几个，retry = 没全发出去再试几次） */
    private static final class Delivery {
        final String target;
        final int port;
        final byte[] packet;
        final int times;
        final int retry;

        Delivery(String target, int port, byte[] packet, int times, int retry) {
            this.target = target;
            this.port = port;
            this.packet = packet;
            this.times = Math.max(1, times);
            this.retry = Math.max(0, retry);
        }
    }

    /** 重试之间的间隔：WOL 没有回执，只能隔一会儿再打一轮 */
    private static final int RETRY_INTERVAL_MS = 200;

    /**
     * 按这台电脑配的「局域网电源路由」发一个动作。**必须在后台线程调用**。
     *
     * <p>包体 = **标准的 102 字节魔术包**（6×0xFF + MAC×16），一个自定义字节都
     * 不带 —— 端口就是指令：目标机上的 sol 按端口执行对应动作，所以同一个动作
     * 可以配多条路由（多个广播地址/端口），也可以只挑其中几块网卡的 MAC。
     * MAC 是所有动作的必填项（没有 MAC 就没有包体）。
     *
     * @param action wake / reboot / shutdown / sleep
     * @return 给用户看的提示；这台电脑没给该动作配路由时返回 {@code null}
     *         —— 调用方回落到老路（唤醒走标准 WOL，其余走电脑端 /api/power）
     */
    static String sendRoutes(Context context, ServerProfile profile, String action,
                             SshSessionManager.Prompt prompt) throws Exception {
        String wanted = ServerProfile.normalizePowerAction(action);
        List<ServerProfile.PowerRoute> routes = profile.routesFor(wanted);
        if (routes.isEmpty()) {
            return null;
        }
        boolean waking = ServerProfile.POWER_WAKE.equals(wanted);
        List<Delivery> deliveries = new ArrayList<>();
        List<String> described = new ArrayList<>();
        List<String> problems = new ArrayList<>();

        for (ServerProfile.PowerRoute route : routes) {
            List<String> macs = profile.routeMacs(route);
            if (macs.isEmpty()) {
                problems.add("这条路由没配 MAC，标准魔术包发不出去");
                continue;
            }
            List<String> targets = ServerProfile.text(route.target).isEmpty()
                    ? broadcastTargets(profile.host())
                    : Collections.singletonList(ServerProfile.text(route.target));
            int port = route.effectivePort();
            if (waking) {
                // 唤醒：每块网卡各发一份魔术包（不知道现在插的是哪根网线），
                // 每块发 burst 个（默认 10）+ 没发出去重试 retry 次（默认 10）
                for (String mac : macs) {
                    byte[] packet = magicPacket(mac);
                    for (String target : targets) {
                        deliveries.add(new Delivery(target, port, packet,
                                route.effectiveBurst(), route.effectiveRetry()));
                    }
                }
            } else {
                // 其余动作：只发一个目标，但发包数 / 重试次数照样按路由走 ——
                // 默认 1 包 / 1 重试；改大了就是「发几次就执行几次」，由用户决定
                byte[] packet = magicPacket(macs.get(0));
                deliveries.add(new Delivery(targets.get(0), port, packet,
                        route.effectiveBurst(), route.effectiveRetry()));
            }
            StringBuilder summary = new StringBuilder();
            for (String target : targets) {
                if (summary.length() > 0) {
                    summary.append(" / ");
                }
                summary.append(target);
            }
            described.add(ServerProfile.powerActionLabel(wanted) + " → " + summary + ":" + port
                    + (waking
                        ? "（" + macs.size() + " 个 MAC × " + route.effectiveBurst() + " 包 / 重试 "
                            + route.effectiveRetry() + " 次）"
                        : "（" + route.effectiveBurst() + " 包 / 重试 "
                            + route.effectiveRetry() + " 次）"));
        }

        if (deliveries.isEmpty()) {
            throw new Exception("没有可发的电源包：" + (problems.isEmpty()
                    ? "这些路由都被关掉了" : join(problems, "；")));
        }

        String jumpId = ServerProfile.text(profile.jumpServerId);
        String whereFrom;
        int sent = 0;
        if (jumpId.isEmpty()) {
            whereFrom = "本机";
            sent = sendLocal(deliveries);
        } else {
            JumpServer jump = JumpStore.findJump(JumpStore.loadJumps(context), jumpId);
            if (jump == null) {
                throw new Exception("跳板机不存在（可能已删除），请到「服务器」页检查");
            }
            if (!jump.isSsh()) {
                throw new Exception("跳板机是 Telnet，发不了局域网电源包 —— 换一台 SSH 服务器");
            }
            // 「重试次数」= 重试发包；走跳板机时每一轮重试都会重新连接跳板机
            // （ensureSession 发现断线就重连），直到次数用完 —— 重试次数就是跳板机重连次数。
            int maxRetry = 0;
            for (Delivery delivery : deliveries) {
                maxRetry = Math.max(maxRetry, delivery.retry);
            }
            Exception lastError = null;
            boolean done = false;
            for (int attempt = 0; attempt <= maxRetry && !done; attempt++) {
                try {
                    sent = sendViaJump(context, jump, deliveries, prompt);
                    done = true;
                } catch (Exception error) {
                    String message = ServerProfile.text(error.getMessage());
                    if (message.contains("python3")) {
                        throw error;   // 跳板机没装 python3 不会自愈，重试没有意义
                    }
                    lastError = error;
                    if (attempt < maxRetry) {
                        Thread.sleep(RETRY_INTERVAL_MS);
                    }
                }
            }
            if (!done) {
                throw lastError != null ? lastError : new Exception("跳板机没能发出电源包");
            }
            whereFrom = "跳板机「" + (ServerProfile.text(jump.name).isEmpty() ? jump.host : jump.name) + "」";
        }
        if (sent == 0) {
            throw new Exception(whereFrom + "一个包都没发出去：可能在隔离网络、防火墙拦了 UDP 广播，"
                    + "或者广播地址填错了（手机要能访问到那个网段）");
        }

        String label = ServerProfile.powerActionLabel(wanted);
        StringBuilder message = new StringBuilder("已从").append(whereFrom).append("按路由发送")
                .append(label).append("：").append(join(described, " · "));
        if (!waking) {
            message.append("。这个动作靠目标机上的 sol 监听该端口执行 —— 没装 sol 不会有反应");
        }
        if (!problems.isEmpty()) {
            message.append("（跳过 ").append(problems.size()).append(" 条：")
                    .append(join(problems, "；")).append("）");
        }
        return message.toString();
    }

    /** 手机自己发（和目标机同网段时有效）：每条按 times 发，没全发出去再试，最多 retry 次 */
    private static int sendLocal(List<Delivery> deliveries) throws Exception {
        DatagramSocket socket = null;
        int sent = 0;
        try {
            socket = new DatagramSocket();
            socket.setBroadcast(true);
            for (Delivery delivery : deliveries) {
                InetAddress address;
                try {
                    address = InetAddress.getByName(delivery.target);
                } catch (Exception ignored) {
                    continue; // 这条目标解析不了，别的照发
                }
                for (int attempt = 0; attempt <= delivery.retry; attempt++) {
                    int round = 0;
                    for (int i = 0; i < delivery.times; i++) {
                        try {
                            socket.send(new DatagramPacket(
                                    delivery.packet, delivery.packet.length, address, delivery.port));
                            round++;
                        } catch (Exception ignored) {
                            // 某个目标不可达无所谓，别的照发
                        }
                    }
                    sent += round;
                    if (round == delivery.times || attempt == delivery.retry) {
                        break;
                    }
                    Thread.sleep(RETRY_INTERVAL_MS);
                }
            }
        } catch (Exception error) {
            throw new Exception("局域网广播失败：" + describe(error)
                    + "（手机和这台电脑不在同一网段时用不了，请配跳板机）");
        } finally {
            if (socket != null) {
                socket.close();
            }
        }
        return sent;
    }

    /** 经跳板机发：登录跳板机跑 python，按计划表把包发到内网 */
    private static int sendViaJump(Context context, JumpServer jump, List<Delivery> deliveries,
                                   SshSessionManager.Prompt prompt) throws Exception {
        JSONArray plan = new JSONArray();
        for (Delivery delivery : deliveries) {
            JSONObject item = new JSONObject();
            item.put("targets", new JSONArray().put(delivery.target));
            item.put("port", delivery.port);
            item.put("packet", hex(delivery.packet));
            item.put("times", delivery.times);
            item.put("retry", delivery.retry);
            plan.put(item);
        }
        // 计划表用 base64(JSON) 传：包体是二进制，命令行里只能放十六进制
        String encodedPlan = Base64.encodeToString(
                plan.toString().getBytes(StandardCharsets.UTF_8), Base64.NO_WRAP);
        String encodedScript = Base64.encodeToString(
                PY_POWER.getBytes(StandardCharsets.UTF_8), Base64.NO_WRAP);
        String command = "if command -v python3 >/dev/null 2>&1; then "
                + "python3 -c \"import base64,sys;exec(base64.b64decode('" + encodedScript
                + "').decode())\" " + encodedPlan
                + "; else echo ANYBOOT-POWER-NOTOOL; fi";

        Session session = SshSessionManager.shared().ensureSession(context, jump, prompt);
        String output = exec(session, command);
        if (output.contains("ANYBOOT-POWER-NOTOOL")) {
            throw new Exception("跳板机上没有 python3 —— 装一个（apt install python3）或换台跳板机");
        }
        int index = output.indexOf("ANYBOOT-POWER-OK");
        if (index < 0) {
            String detail = ServerProfile.text(output);
            throw new Exception("跳板机没能发出电源包"
                    + (detail.isEmpty() ? "" : "：" + truncate(detail, 160)));
        }
        String rest = output.substring(index + "ANYBOOT-POWER-OK".length()).trim();
        try {
            return Integer.parseInt(rest.split("\\s+")[0]);
        } catch (Exception ignored) {
            return 1;   // 有 OK 但没能解析出条数：当作发出去了
        }
    }

    /** 包体转十六进制（JSON 里只能放文本） */
    private static String hex(byte[] bytes) {
        StringBuilder builder = new StringBuilder(bytes.length * 2);
        for (byte value : bytes) {
            builder.append(Character.forDigit((value >> 4) & 0xF, 16));
            builder.append(Character.forDigit(value & 0xF, 16));
        }
        return builder.toString();
    }

    private static String join(List<String> values, String separator) {
        StringBuilder builder = new StringBuilder();
        for (String value : values) {
            if (builder.length() > 0) {
                builder.append(separator);
            }
            builder.append(value);
        }
        return builder.toString();
    }

    /** 跳板机侧的 python：按计划表逐条发（不依赖装了 wakeonlan / etherwake） */
    private static final String PY_POWER =
            "import base64,json,socket,sys,time\n"
                    + "plan=json.loads(base64.b64decode(sys.argv[1]).decode('utf-8'))\n"
                    + "s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)\n"
                    + "s.setsockopt(socket.SOL_SOCKET,socket.SO_BROADCAST,1)\n"
                    + "sent=0\n"
                    + "for item in plan:\n"
                    + "    packet=bytes.fromhex(item['packet'])\n"
                    + "    times=max(1,int(item.get('times',1)))\n"
                    + "    retry=int(item.get('retry',0))\n"
                    + "    for target in item['targets']:\n"
                    + "        for attempt in range(retry+1):\n"
                    + "            ok=0\n"
                    + "            for _ in range(times):\n"
                    + "                try:\n"
                    + "                    s.sendto(packet,(target,int(item['port'])))\n"
                    + "                    ok+=1\n"
                    + "                except Exception:\n"
                    + "                    pass\n"
                    + "            sent+=ok\n"
                    + "            if ok==times:\n"
                    + "                break\n"
                    + "            time.sleep(0.2)\n"
                    + "print('ANYBOOT-POWER-OK %d' % sent)\n"
                    + "s.close()\n";

    // ------------------------------------------------------------------
    // 本机局域网广播
    // ------------------------------------------------------------------

    private static String wakeLocal(ServerProfile profile, List<String> macs) throws Exception {
        List<String> targets = broadcastTargets(profile.host());
        DatagramSocket socket = null;
        try {
            socket = new DatagramSocket();
            socket.setBroadcast(true);
            for (String mac : macs) {
                byte[] packet = magicPacket(mac);
                for (String target : targets) {
                    InetAddress address;
                    try {
                        address = InetAddress.getByName(target);
                    } catch (Exception ignored) {
                        continue;
                    }
                    // 9 端口是标准 WOL：按默认数量多发几个（丢一两个也能醒）；
                    // 7 端口给老网卡，发一个兜底就够 —— 别把局域网刷满。
                    for (int port : PORTS) {
                        int times = port == 9 ? ServerProfile.DEFAULT_WAKE_BURST : 1;
                        for (int i = 0; i < times; i++) {
                            try {
                                socket.send(new DatagramPacket(
                                        packet, packet.length, address, port));
                            } catch (Exception ignored) {
                                // 某个广播地址不可达无所谓，别的照发
                            }
                        }
                    }
                }
            }
        } catch (Exception error) {
            throw new Exception("局域网广播失败：" + describe(error)
                    + "（手机和这台电脑不在同一网段时用不了，请配跳板机）");
        } finally {
            if (socket != null) {
                socket.close();
            }
        }
        return "已向局域网广播唤醒包（" + macs.size() + " 个 MAC），等 10~30 秒看它能不能连上";
    }

    /**
     * 广播目标：**先目标地址所在 /24 的子网广播，再全局广播**。
     *
     * <p>顺序有意义：非唤醒动作只发第一个目标（同一动作发两遍会被 sol 执行两遍），
     * 而子网广播比 255.255.255.255 更容易真的送到（很多路由器不转发全局广播）。
     */
    private static List<String> broadcastTargets(String address) {
        List<String> targets = new ArrayList<>();
        String host = ServerProfile.text(address);
        int colon = host.lastIndexOf(':');
        if (colon > 0) {
            host = host.substring(0, colon);
        }
        if (host.matches("^(\\d{1,3}\\.){3}\\d{1,3}$")) {
            targets.add(host.substring(0, host.lastIndexOf('.')) + ".255");
        }
        targets.add("255.255.255.255");
        return targets;
    }

    /** 标准魔术包：6 个 0xFF + 目标 MAC 重复 16 次（共 102 字节） */
    private static byte[] magicPacket(String mac) throws Exception {
        String hex = ServerProfile.normalizeMac(mac).replace(":", "");
        if (hex.length() != 12) {
            throw new Exception("MAC 格式不对：" + mac);
        }
        byte[] packet = new byte[6 + 16 * 6];
        for (int i = 0; i < 6; i++) {
            packet[i] = (byte) 0xFF;
        }
        for (int repeat = 0; repeat < 16; repeat++) {
            for (int i = 0; i < 6; i++) {
                packet[6 + repeat * 6 + i] = (byte) Integer.parseInt(hex.substring(i * 2, i * 2 + 2), 16);
            }
        }
        return packet;
    }

    // ------------------------------------------------------------------
    // 经跳板机（SSH）
    // ------------------------------------------------------------------

    private static String wakeViaJump(Context context, ServerProfile profile, String jumpId,
                                      List<String> macs, SshSessionManager.Prompt prompt)
            throws Exception {
        JumpServer jump = JumpStore.findJump(JumpStore.loadJumps(context), jumpId);
        if (jump == null) {
            throw new Exception("跳板机不存在（可能已删除），请到「服务器」页检查");
        }
        if (!jump.isSsh()) {
            throw new Exception("跳板机是 Telnet，发不了唤醒命令 —— 换一台 SSH 服务器");
        }
        String targets = broadcastTargets(profile.host()).size() > 1
                ? broadcastTargets(profile.host()).get(1)
                : "255.255.255.255";
        String command = wolCommand(macs, targets);

        Session session = SshSessionManager.shared().ensureSession(context, jump, prompt);
        String output = exec(session, command);
        if (output.contains("ANYBOOT-WOL-NOTOOL")) {
            throw new Exception("跳板机上没有 python3 / wakeonlan / etherwake —— "
                    + "装一个（apt install wakeonlan）或换台跳板机");
        }
        if (!output.contains("ANYBOOT-WOL-OK")) {
            String detail = ServerProfile.text(output);
            throw new Exception("跳板机没能发出唤醒包"
                    + (detail.isEmpty() ? "" : "：" + truncate(detail, 160)));
        }
        String label = ServerProfile.text(jump.name).isEmpty() ? jump.host : jump.name;
        return "已通过跳板机「" + label + "」广播唤醒包（" + macs.size()
                + " 个 MAC，广播地址 " + targets + "），等 10~30 秒看它能不能连上";
    }

    /**
     * 跳板机上跑的 shell 片段。
     *
     * <p>python3 走 base64 传脚本：整段脚本里带引号、反斜杠、换行，直接内联进
     * 命令行会被远端 shell 吃掉一层引号（踩过）。base64 只有字母数字和 +/=，
     * 在单引号里绝对安全。
     */
    private static String wolCommand(List<String> macs, String broadcast) {
        // android.util.Base64 的 NO_WRAP：不折行，避免 base64 里出现 \n 把命令切断
        String encoded = Base64.encodeToString(
                PY_SCRIPT.getBytes(StandardCharsets.UTF_8), Base64.NO_WRAP);
        StringBuilder joined = new StringBuilder();
        for (String mac : macs) {
            if (joined.length() > 0) {
                joined.append(' ');
            }
            joined.append(mac);
        }
        String python = "python3 -c \"import base64;exec(base64.b64decode('" + encoded
                + "').decode('utf-8'))\" " + broadcast + " " + joined;
        String fallback = "for m in " + joined + "; do wakeonlan \"$m\"; done";
        String etherwake = "for m in " + joined + "; do etherwake \"$m\"; done";
        return "if command -v python3 >/dev/null 2>&1; then " + python
                + " && echo ANYBOOT-WOL-OK; "
                + "elif command -v wakeonlan >/dev/null 2>&1; then " + fallback
                + " && echo ANYBOOT-WOL-OK; "
                + "elif command -v etherwake >/dev/null 2>&1; then " + etherwake
                + " && echo ANYBOOT-WOL-OK; "
                + "else echo ANYBOOT-WOL-NOTOOL; fi";
    }

    /** 在已建立的 SSH 会话上跑一条 exec 命令，返回 stdout+stderr */
    private static String exec(Session session, String command) throws Exception {
        ChannelExec channel = (ChannelExec) session.openChannel("exec");
        channel.setCommand(command);
        ByteArrayOutputStream errors = new ByteArrayOutputStream();
        channel.setErrStream(errors);
        // ⚠️ 必须先 connect 再 getInputStream：jsch 的 Channel 在未连接时
        // getInputStream() 直接抛 "channel is not opened"（不是返回空流）。
        channel.connect(8000);
        InputStream input = channel.getInputStream();
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        byte[] buffer = new byte[1024];
        long deadline = System.currentTimeMillis() + 15000;
        try {
            while (true) {
                while (input.available() > 0) {
                    int read = input.read(buffer);
                    if (read > 0) {
                        output.write(buffer, 0, read);
                    }
                }
                if (channel.isClosed()) {
                    break;
                }
                if (System.currentTimeMillis() > deadline) {
                    break;
                }
                Thread.sleep(100);
            }
            output.write(errors.toByteArray());
            return output.toString("UTF-8");
        } finally {
            channel.disconnect();
        }
    }

    private static String truncate(String value, int limit) {
        String text = ServerProfile.text(value).replace('\n', ' ');
        return text.length() <= limit ? text : text.substring(0, limit) + "…";
    }

    private static String describe(Exception error) {
        String message = error.getMessage();
        return message == null || message.isEmpty() ? error.getClass().getSimpleName() : message;
    }
}
