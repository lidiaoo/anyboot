package com.anyboot.remote;

import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.net.URLEncoder;

/**
 * 电源操作（重启 / 关机 / 睡眠）—— 走电脑端 HTTP 接口 {@code /api/power}。
 *
 * <p>和网页里的电源按钮同一条路：电脑端收到请求后按自己的平台能力执行
 * （{@code power.rs} 的 Reboot / Shutdown / Sleep），返回「已提交」。
 * App 只负责发请求，不自己判断能不能关机。
 *
 * <p>注意「睡眠」发出去之后这台电脑的 HTTP 服务就断了：心跳会显示离线、
 * 页面会报连不上，都是预期内的。要让它回来只能靠 {@link WakeOnLan}（MAC 唤醒）。
 */
final class PowerOps {

    static final String REBOOT = "reboot";
    static final String SHUTDOWN = "shutdown";
    static final String SLEEP = "sleep";

    /** 电源请求的超时：关机途中响应可能慢，给宽一点 */
    private static final int CONNECT_TIMEOUT_MS = 4000;
    private static final int READ_TIMEOUT_MS = 15000;

    private PowerOps() {
    }

    /** 动作的中文名（提示文案用） */
    static String label(String action) {
        switch (action) {
            case REBOOT:
                return "重启";
            case SHUTDOWN:
                return "关机";
            case SLEEP:
                return "睡眠";
            default:
                return action;
        }
    }

    /**
     * 发一次电源请求。**必须在后台线程调用**。
     *
     * @param address 已经解析好的可访问地址（直连 host:port，或跳板机隧道的 127.0.0.1:端口）
     * @return 电脑端返回的提示语
     * @throws Exception 连不上 / token 不对 / 电脑端拒绝
     */
    static String send(String address, String token, String action) throws Exception {
        String target = ServerProfile.text(address);
        if (target.isEmpty()) {
            throw new Exception("这台电脑还没有可用地址");
        }
        StringBuilder url = new StringBuilder("http://").append(target)
                .append("/api/power?action=").append(URLEncoder.encode(action, "UTF-8"));
        String secret = ServerProfile.text(token);
        if (!secret.isEmpty()) {
            url.append("&token=").append(URLEncoder.encode(secret, "UTF-8"));
        }

        HttpURLConnection connection = null;
        try {
            connection = (HttpURLConnection) new URL(url.toString()).openConnection();
            connection.setRequestMethod("GET");
            connection.setConnectTimeout(CONNECT_TIMEOUT_MS);
            connection.setReadTimeout(READ_TIMEOUT_MS);
            connection.setInstanceFollowRedirects(false);
            int code = connection.getResponseCode();
            String body = read(code >= 400 ? connection.getErrorStream() : connection.getInputStream());
            if (code == 401) {
                throw new Exception("电脑端不认这个 token（401）—— 到「编辑服务端」核对访问令牌");
            }
            if (code >= 400) {
                throw new Exception(messageOf(body, "电脑端返回 " + code));
            }
            return messageOf(body, "已提交" + label(action));
        } finally {
            if (connection != null) {
                connection.disconnect();
            }
        }
    }

    /** 从响应体里挑一句能看的话：优先 message，其次 error，都没有就用兜底文案 */
    private static String messageOf(String body, String fallback) {
        try {
            JSONObject object = new JSONObject(ServerProfile.text(body));
            String message = ServerProfile.text(object.optString("message", ""));
            if (!message.isEmpty()) {
                return message;
            }
            String error = ServerProfile.text(object.optString("error", ""));
            if (!error.isEmpty()) {
                return error;
            }
        } catch (Exception ignored) {
            // 不是 JSON（老版本 / 网关错误页）：用兜底文案
        }
        return fallback;
    }

    private static String read(InputStream stream) throws Exception {
        if (stream == null) {
            return "";
        }
        try (InputStream input = stream; ByteArrayOutputStream buffer = new ByteArrayOutputStream()) {
            byte[] chunk = new byte[2048];
            int count;
            while ((count = input.read(chunk)) > 0) {
                buffer.write(chunk, 0, count);
                if (buffer.size() > 16 * 1024) {
                    break;   // 电源接口只回一句，防一手畸形容器
                }
            }
            return buffer.toString("UTF-8");
        }
    }
}
