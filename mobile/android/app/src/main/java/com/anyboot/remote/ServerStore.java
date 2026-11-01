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
    private static final String KEY_TRANSPORT = "transport";
    private static final String KEY_MIGRATED = "migrated_multi_server";
    private static final String KEY_SORT_MODE = "server_sort_mode";

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
                        if (!profile.addresses.isEmpty()) {
                            list.add(profile);
                        }
                    }
                }
            } catch (Exception ignored) {
                // 配置损坏就当没有，别让 App 起不来
            }
        }
        migrateLegacy(context, list);
        sort(context, list);
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
            profile.addAddress(host);
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

    /**
     * 传输方式偏好："auto"（默认）/ "ws"（只用推送）/ "http"（只用轮询）。
     *
     * <p>存在 App 侧而不是只存网页端：网页端的 localStorage 按来源站点存，
     * 这台电脑换网卡就会换地址（= 换来源），偏好会丢。加载页面时以
     * {@code ?transport=} 追加到 URL 上交给网页决定走哪条通道。
     */
    static String transport(Context context) {
        String value = value(prefs(context).getString(KEY_TRANSPORT, "")).trim();
        return value.isEmpty() ? "auto" : value;
    }

    static void setTransport(Context context, String value) {
        prefs(context).edit().putString(KEY_TRANSPORT, value == null ? "auto" : value).apply();
    }

    /** 排序模式："custom"（默认，用户拖出来的顺序）/ "name"（A→Z）/ "recent"（最近连接优先） */
    static String sortMode(Context context) {
        String value = value(prefs(context).getString(KEY_SORT_MODE, "")).trim();
        return value.isEmpty() ? "custom" : value;
    }

    static void setSortMode(Context context, String value) {
        String mode = "name".equals(value) || "recent".equals(value) ? value : "custom";
        prefs(context).edit().putString(KEY_SORT_MODE, mode).apply();
    }

    /** 按当前模式排序（与 PC 端 cqSortRemotes 同一套语义） */
    static void sort(Context context, List<ServerProfile> list) {
        String mode = sortMode(context);
        if ("recent".equals(mode)) {
            sortRecent(list);
        } else if ("name".equals(mode)) {
            sortName(list);
        } else {
            sortCustom(list);
        }
    }

    /** 自定义顺序：有 sortIndex 的按序号升序，没排过的按名称接在后面（与 Rust sort_servers_custom 一致） */
    static void sortCustom(List<ServerProfile> list) {
        Collections.sort(list, (left, right) -> {
            long a = left.sortIndex;
            long b = right.sortIndex;
            if (a >= 0 || b >= 0) {
                if (a < 0) return 1;
                if (b < 0) return -1;
                if (a != b) return Long.compare(a, b);
            }
            return nameThenId(left, right);
        });
    }

    /** 显示名 A→Z */
    static void sortName(List<ServerProfile> list) {
        Collections.sort(list, (left, right) -> nameThenId(left, right));
    }

    /** 最近连接优先：刚连过的排前面，没用过/久没用的按名称排后面 */
    static void sortRecent(List<ServerProfile> list) {
        Collections.sort(list, (left, right) -> {
            if (left.lastUsedAt != right.lastUsedAt) {
                return Long.compare(right.lastUsedAt, left.lastUsedAt);
            }
            return nameThenId(left, right);
        });
    }

    private static int nameThenId(ServerProfile left, ServerProfile right) {
        int byName = left.sortKey().compareToIgnoreCase(right.sortKey());
        return byName != 0 ? byName : left.id.compareToIgnoreCase(right.id);
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

    private static String value(String raw) {
        return raw == null ? "" : raw;
    }
}
