package com.anyboot.remote;

import android.os.SystemClock;

import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;

/**
 * 探测电脑端的 `GET /health`（免鉴权探活接口）。
 *
 * <p>用途只有一个：拿到 `machineId` —— 一台电脑可能有多块网卡、多个 IP，
 * 客户端靠它认出「这几条地址其实是同一台机器」，从而把地址并进同一条记录、
 * 只保留一份页面（而不是一个网卡一个标签）。
 *
 * <p>老版本服务端不返回这个字段，这时标识是空串，客户端会退回按「分组名相同」
 * 判断同一台电脑 —— 所以这个探测失败不影响任何功能，只是少一点自动识别。
 */
final class HealthProbe {

    /** 单次探测超时：这是保存地址后顺带做的事，不能拖住用户 */
    private static final int TIMEOUT_MS = 1500;

    /** `/health` 里认出「这是哪台电脑」的两个字段 */
    static final class Result {
        final String machineId;
        final String machineName;

        Result(String machineId, String machineName) {
            this.machineId = machineId;
            this.machineName = machineName;
        }
    }

    private HealthProbe() {
    }

    /** 阻塞式探测；地址不通 / 不是 AnyBoot 时返回 null。**必须在后台线程调用。** */
    static Result fetch(String host) {
        return fetch(host, TIMEOUT_MS);
    }

    /** 同上，超时自己带 —— 心跳要比保存时的顺带探测宽容一点，弱网别误报离线 */
    static Result fetch(String host, int timeoutMs) {
        String target = ServerProfile.text(host);
        if (target.isEmpty()) {
            return null;
        }
        HttpURLConnection connection = null;
        try {
            connection = (HttpURLConnection) new URL("http://" + target + "/health").openConnection();
            connection.setRequestMethod("GET");
            connection.setConnectTimeout(timeoutMs);
            connection.setReadTimeout(timeoutMs);
            connection.setInstanceFollowRedirects(false);
            if (connection.getResponseCode() != 200) {
                return null;
            }
            return parse(read(connection.getInputStream()));
        } catch (Exception ignored) {
            // 连不上 / 超时 / 地址不合法：都当「没探到」，别打扰用户
            return null;
        } finally {
            if (connection != null) {
                connection.disconnect();
            }
        }
    }

    /**
     * 心跳探测：只看通不通，通了返回往返毫秒，不通返回 -1。**必须在后台线程调用。**
     *
     * <p>跟 {@link #fetch} 的分工：fetch 是「保存地址后顺手认一次机器」要解析内容，
     * 10 秒心跳走这里 —— 只要状态码，开销最小。
     */
    static long ping(String host, int timeoutMs) {
        String target = ServerProfile.text(host);
        if (target.isEmpty()) {
            return -1;
        }
        HttpURLConnection connection = null;
        long started = SystemClock.uptimeMillis();
        try {
            connection = (HttpURLConnection) new URL("http://" + target + "/health").openConnection();
            connection.setRequestMethod("GET");
            connection.setConnectTimeout(timeoutMs);
            connection.setReadTimeout(timeoutMs);
            connection.setInstanceFollowRedirects(false);
            if (connection.getResponseCode() != 200) {
                return -1;
            }
            // 响应体不读也行，但读掉再断开能复用连接池；只读一小口防畸形容器
            InputStream input = connection.getInputStream();
            byte[] chunk = new byte[1024];
            // noinspection ResultOfMethodCallIgnored
            input.read(chunk);
            return SystemClock.uptimeMillis() - started;
        } catch (Exception ignored) {
            return -1;
        } finally {
            if (connection != null) {
                connection.disconnect();
            }
        }
    }

    /**
     * 远程地址探测（公网隧道 / ts.net，https）：给完整 origin，
     * {@link HttpURLConnection} 原生支持 TLS。超时放宽 —— 公网 RTT 大，
     * 按局域网的 1.5s 判会大量误报。通了返回往返毫秒，不通 -1。
     * **必须在后台线程调用。**
     */
    static long pingRemote(String origin, int timeoutMs) {
        String target = ServerProfile.normalizeRemoteOrigin(origin);
        if (target.isEmpty()) {
            return -1;
        }
        HttpURLConnection connection = null;
        long started = SystemClock.uptimeMillis();
        try {
            connection = (HttpURLConnection) new URL(target + "/health").openConnection();
            connection.setRequestMethod("GET");
            connection.setConnectTimeout(timeoutMs);
            connection.setReadTimeout(timeoutMs);
            connection.setInstanceFollowRedirects(false);
            if (connection.getResponseCode() != 200) {
                return -1;
            }
            InputStream input = connection.getInputStream();
            byte[] chunk = new byte[1024];
            // noinspection ResultOfMethodCallIgnored
            input.read(chunk);
            return SystemClock.uptimeMillis() - started;
        } catch (Exception ignored) {
            return -1;
        } finally {
            if (connection != null) {
                connection.disconnect();
            }
        }
    }

    /** 解析 `/health` 响应体（纯逻辑，便于单测）。字段缺失 / 不是 JSON 时给 null。 */
    static Result parse(String body) {
        try {
            JSONObject object = new JSONObject(ServerProfile.text(body));
            return new Result(
                    ServerProfile.text(object.optString("machineId", "")),
                    ServerProfile.text(object.optString("machineName", "")));
        } catch (Exception ignored) {
            return null;
        }
    }

    private static String read(InputStream stream) throws Exception {
        try (InputStream input = stream; ByteArrayOutputStream buffer = new ByteArrayOutputStream()) {
            byte[] chunk = new byte[4096];
            int count;
            while ((count = input.read(chunk)) > 0) {
                buffer.write(chunk, 0, count);
                if (buffer.size() > 64 * 1024) {
                    break;   // /health 只有几十字节，防一手畸形容器
                }
            }
            return buffer.toString("UTF-8");
        }
    }
}
