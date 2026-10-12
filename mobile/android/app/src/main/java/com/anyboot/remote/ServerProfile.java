package com.anyboot.remote;

import android.graphics.Color;
import android.net.Uri;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.UUID;

/**
 * 一个服务端条目 = 一台电脑上的一个 AnyBoot 实例（一个 IP:端口）。
 *
 * <p>同一个分组里可以放**多台网卡 / 多次 DHCP 拿到多个 IP** 的同一台电脑：
 * 用户在标签上看到的是「组名 · 服务端名」，一眼知道这几个标签其实是同一台机器。
 */
final class ServerProfile {

    /** 电脑端「远程控制」的默认端口 */
    static final int DEFAULT_PORT = 12300;

    String id;
    /** 分组名（用来区分不同的电脑）；空串表示「未分组」 */
    String group;
    /** 服务端名；空串表示用地址兜底 */
    String name;
    /** 归一化后的 host:port */
    String host;
    /** 访问令牌；可空 */
    String token;

    ServerProfile() {
        this.id = UUID.randomUUID().toString();
        this.group = "";
        this.name = "";
        this.host = "";
        this.token = "";
    }

    /** 标签上显示的服务端名（没起名字就用地址） */
    String displayName() {
        String value = name == null ? "" : name.trim();
        return value.isEmpty() ? host : value;
    }

    /** 标签文本：组名 + 服务端名（未分组时只显示服务端名） */
    String label() {
        String groupName = group == null ? "" : group.trim();
        return groupName.isEmpty() ? displayName() : groupName + " · " + displayName();
    }

    /** 带 token 的访问地址（token 会做百分号编码） */
    String url() {
        String base = "http://" + host + "/";
        String value = token == null ? "" : token.trim();
        return value.isEmpty() ? base : base + "?token=" + Uri.encode(value);
    }

    /** 排序键：先按分组（未分组排最后），再按显示名 */
    String sortKey() {
        String groupName = group == null ? "" : group.trim();
        String prefix = groupName.isEmpty() ? "\uffff" : groupName;
        return prefix + "\u0000" + displayName() + "\u0000" + host;
    }

    /**
     * 分组配色：同一个分组恒定同一个颜色（取组名的哈希定色相），
     * 让人扫一眼标签栏就能看出「哪几个标签是同一台电脑」。
     */
    int groupColor() {
        String groupName = group == null ? "" : group.trim();
        if (groupName.isEmpty()) {
            return 0xFFB9BFC9;
        }
        int hash = 0;
        for (int i = 0; i < groupName.length(); i++) {
            hash = hash * 31 + groupName.charAt(i);
        }
        float hue = Math.abs(hash % 360);
        return Color.HSVToColor(new float[] { hue, 0.45f, 0.78f });
    }

    ServerProfile copy() {
        ServerProfile other = new ServerProfile();
        other.id = id;
        other.group = group;
        other.name = name;
        other.host = host;
        other.token = token;
        return other;
    }

    JSONObject toJson() throws JSONException {
        JSONObject object = new JSONObject();
        object.put("id", id);
        object.put("group", group == null ? "" : group);
        object.put("name", name == null ? "" : name);
        object.put("host", host == null ? "" : host);
        object.put("token", token == null ? "" : token);
        return object;
    }

    static ServerProfile fromJson(JSONObject object) {
        ServerProfile profile = new ServerProfile();
        String id = object.optString("id", "");
        if (!id.isEmpty()) {
            profile.id = id;
        }
        profile.group = object.optString("group", "");
        profile.name = object.optString("name", "");
        profile.host = object.optString("host", "");
        profile.token = object.optString("token", "");
        return profile;
    }

    /**
     * 用户可能粘贴整条 URL（http://192.168.0.120:12300/?token=xxx），
     * 也可能只写 IP，统一归一成 host:port。
     */
    static String normalizeHost(String raw) {
        String value = raw == null ? "" : raw.trim();
        if (value.isEmpty()) {
            return "";
        }
        int scheme = value.indexOf("://");
        if (scheme >= 0) {
            value = value.substring(scheme + 3);
        }
        int slash = value.indexOf('/');
        if (slash >= 0) {
            value = value.substring(0, slash);
        }
        int question = value.indexOf('?');
        if (question >= 0) {
            value = value.substring(0, question);
        }
        value = value.trim();
        if (value.isEmpty()) {
            return "";
        }
        if (!value.contains(":")) {
            value = value + ":" + DEFAULT_PORT;
        }
        return value;
    }

    /** 从粘贴的 URL 里顺手把 token 挖出来。 */
    static String tokenFromUrl(String raw) {
        if (raw == null || raw.indexOf("token=") < 0) {
            return "";
        }
        try {
            String token = Uri.parse(raw.trim()).getQueryParameter("token");
            return token == null ? "" : token.trim();
        } catch (Exception ignored) {
            return "";
        }
    }
}
