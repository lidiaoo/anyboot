package com.anyboot.remote;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * 活着的多会话终端注册表（静态持有，Activity 重建不丢）。
 *
 * <p>一个服务器同时最多一条会话（重复点「打开终端」回到已有会话）；
 * 「会话页」展示的就是这里的内容 + {@link JumpStore} 的历史。
 */
final class TerminalRegistry {

    interface Observer {
        /** 会话集合或状态有变（建立 / 关闭）—— 会话页刷新 */
        void onChanged();
    }

    private static final Map<String, TerminalSession> LIVE = new LinkedHashMap<>();
    private static Observer observer;

    private TerminalRegistry() {
    }

    static void setObserver(Observer callback) {
        observer = callback;
    }

    static TerminalSession get(String jumpId) {
        synchronized (LIVE) {
            TerminalSession session = LIVE.get(jumpId);
            if (session != null && session.isClosed()) {
                LIVE.remove(jumpId);
                return null;
            }
            return session;
        }
    }

    static List<TerminalSession> all() {
        synchronized (LIVE) {
            List<TerminalSession> list = new ArrayList<>();
            for (TerminalSession session : new ArrayList<>(LIVE.values())) {
                if (session.isClosed()) {
                    LIVE.remove(session.server.id);
                } else {
                    list.add(session);
                }
            }
            return list;
        }
    }

    static void put(TerminalSession session) {
        synchronized (LIVE) {
            LIVE.put(session.server.id, session);
        }
        notifyChanged();
    }

    static void remove(String jumpId) {
        synchronized (LIVE) {
            LIVE.remove(jumpId);
        }
        notifyChanged();
    }

    /** 会话数（侧边栏「服务器」徽标旁边不显示这个；会话页用） */
    static int count() {
        return all().size();
    }

    private static void notifyChanged() {
        Observer callback = observer;
        if (callback != null) {
            callback.onChanged();
        }
    }
}
