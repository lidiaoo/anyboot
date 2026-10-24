package com.anyboot.remote

import androidx.compose.runtime.mutableStateListOf
import android.content.Context

/**
 * 电脑档案的**全进程唯一副本**。
 *
 * 为什么要有这一层：手机上「电脑」页（[MainActivity]）和「电脑列表」页
 * （[ComputerListActivity]）以前各自 `ServerStore.load()` 一份自己的
 * `mutableStateListOf`。两份内存副本导致同一份磁盘数据在两个页面里长得不一样：
 *
 * - 在电脑列表里改了名字/地址/远程接入 → 返回「电脑」页，抽屉里还是旧值
 *   （[MainActivity] 只在 onCreate 读过一次盘，之后再不刷新）；
 * - 更糟的是两边都拿自己那份去 `ServerStore.save(...)` **整表覆盖**，
 *   谁的副本旧，谁一保存就把另一边的改动抹掉。
 *
 * 现在两个页面共用 [servers] 这**一个**列表实例（Compose 的
 * `mutableStateListOf` 是进程级的快照状态，两边同时观察同一个对象），
 * 任何一处增删改另一处立刻重组 —— 这就是用户要的「同一套数据、不同样式展示」。
 *
 * 约定：**任何落盘都必须先改 [servers]，再调 [persist]**；
 * 只有外部（磁盘）变了才调 [reload]。
 */
internal object ServerRepository {

    /** 唯一的电脑列表。两个页面直接读它，谁改都算数。 */
    val servers = mutableStateListOf<ServerProfile>()

    /** 是否已经从磁盘载入过（避免两个页面各自载入把对方的改动冲掉） */
    private var loaded = false

    /** 首次载入；已载入过就什么都不做（保住内存里的现状） */
    fun ensureLoaded(context: Context) {
        if (loaded) return
        reload(context)
    }

    /**
     * 从磁盘重读并原地替换共享列表内容（列表实例不变，观察者自动重组）。
     * 会**重建 ServerProfile 对象**，所以持有旧对象的调用方（如编辑中的表单）
     * 必须按 id 重新取一份 —— [saveServerProfile] 里已经这么做了。
     */
    fun reload(context: Context) {
        val fresh = ServerStore.load(context)
        ServerStore.sort(fresh)
        servers.clear()
        servers.addAll(fresh)
        loaded = true
    }

    /** 唯一的落盘出口：排序 → 写盘（调用方负责先把改动落到 [servers] 上） */
    fun persist(context: Context, list: List<ServerProfile> = servers) {
        ServerStore.sort(servers)
        ServerStore.save(context, list)
    }

    /** 按 id 删一条：共享列表和磁盘一起删（两边页面同时生效） */
    fun remove(context: Context, id: String) {
        servers.removeAll { it.id == id }
        persist(context)
    }

    fun find(id: String): ServerProfile? = servers.firstOrNull { it.id == id }

    /** 已有分组名（表单「分组」下拉的候选） */
    fun knownGroups(): List<String> = ServerStore.knownGroups(servers)
}
