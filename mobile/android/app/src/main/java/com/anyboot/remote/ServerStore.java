package com.anyboot.remote;

import android.content.Context;
import android.content.SharedPreferences;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * 服务端列表的持久化（SharedPreferences + JSON）。
 *
 * <p>1.0 只存了单个 host/token（KEY_HOST / KEY_TOKEN），升级到这里时
 * {@link #migrateLegacy} 会把那一条搬成列表里的第一个服务端，用户无感。
 */
final class ServerStore {

    private static final String PREFS = "anyboot_remote";
    private static final String KEY_SERVERS = "servers";
    private static final String KEY_ACTIVE = "active_id";
    private static final String KEY_CONNECT_ALL = "connect_all";
    private static final String KEY_MIGRATED = "migrated_multi_server";

    private static final String LEGACY_HOST = "host";
    private static final String LEGACY_TOKEN = "token";

    private ServerStore() {
    }

    static SharedPreferences prefs(Context context) {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    /** 读取全部服务端，并按「分组 → 名称」排好序 */
    static List<ServerProfile> load(Context context) {
        List<ServerProfile> list = new ArrayList<>();
        String raw = prefs(context).getString(KEY_SERVERS, "");
        if (raw != null && !raw.trim().isEmpty()) {
            try {
                JSONArray array = new JSONArray(raw);
                for (int i = 0; i < array.length(); i++) {
                    JSONObject object = array.optJSONObject(i);
                    if (object != null) {
                        ServerProfile profile = ServerProfile.fromJson(object);
                        if (!profile.host.isEmpty()) {
                            list.add(profile);
                        }
                    }
                }
            } catch (Exception ignored) {
                // 配置损坏就当没有，别让 App 起不来
            }
        }
        migrateLegacy(context, list);
        sort(list);
        return list;
    }

    static void save(Context context, List<ServerProfile> list) {
        JSONArray array = new JSONArray();
        for (ServerProfile profile : list) {
            try {
                array.put(profile.toJson());
            } catch (Exception ignored) {
                // 单条写不进去就跳过，别的照常存
            }
        }
        prefs(context).edit().putString(KEY_SERVERS, array.toString()).apply();
    }

    /** 把 1.0 的单服务端配置搬进列表（只搬一次） */
    private static void migrateLegacy(Context context, List<ServerProfile> list) {
        SharedPreferences preferences = prefs(context);
        if (preferences.getBoolean(KEY_MIGRATED, false)) {
            return;
        }
        String host = ServerProfile.normalizeHost(preferences.getString(LEGACY_HOST, ""));
        if (!host.isEmpty()) {
            ServerProfile profile = new ServerProfile();
            profile.host = host;
            profile.name = host;
            profile.token = value(preferences.getString(LEGACY_TOKEN, ""));
            list.add(profile);
        }
        preferences.edit()
                .putBoolean(KEY_MIGRATED, true)
                .remove(LEGACY_HOST)
                .remove(LEGACY_TOKEN)
                .apply();
        if (!host.isEmpty()) {
            save(context, list);
        }
    }

    static String activeId(Context context) {
        return value(prefs(context).getString(KEY_ACTIVE, ""));
    }

    static void setActiveId(Context context, String id) {
        prefs(context).edit().putString(KEY_ACTIVE, id == null ? "" : id).apply();
    }

    /** 启动时是否把所有服务端一起连上（多个 WebView 同时在线） */
    static boolean connectAll(Context context) {
        return prefs(context).getBoolean(KEY_CONNECT_ALL, false);
    }

    static void setConnectAll(Context context, boolean value) {
        prefs(context).edit().putBoolean(KEY_CONNECT_ALL, value).apply();
    }

    /** 按「分组 → 名称」排序（未分组排最后） */
    static void sort(List<ServerProfile> list) {
        Collections.sort(list, (left, right) -> left.sortKey().compareToIgnoreCase(right.sortKey()));
    }

    static ServerProfile find(List<ServerProfile> list, String id) {
        if (id == null) {
            return null;
        }
        for (ServerProfile profile : list) {
            if (id.equals(profile.id)) {
                return profile;
            }
        }
        return null;
    }

    /** 已有的分组名（给「分组」输入框做候选） */
    static List<String> knownGroups(List<ServerProfile> list) {
        List<String> groups = new ArrayList<>();
        for (ServerProfile profile : list) {
            String group = profile.group == null ? "" : profile.group.trim();
            if (!group.isEmpty() && !groups.contains(group)) {
                groups.add(group);
            }
        }
        Collections.sort(groups, String::compareToIgnoreCase);
        return groups;
    }

    private static String value(String raw) {
        return raw == null ? "" : raw;
    }
}
