/**
 * 「电脑」档案 store（`remote_*` 命令族）。
 *
 * 这是 hub 的核心数据：**电脑列表**与**电脑页**都读它。
 * 与 `hub` 命令族（SSH/SFTP 服务器）是两套完全独立的档案。
 */

import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { call } from "@/api/tauri";
import type { RemoteBootInfo } from "@/api/types";
import { OFFLINE_RETRY_DEFAULT, OFFLINE_RETRY_KEY, REPROBE_ALWAYS_KEY } from "@/api/constants";

/** 一台电脑的档案（对应 Rust `RemoteStore` 的 server 条目） */
export interface RemoteProfile {
  id: string;
  name: string;
  /** 全部访问地址（`host:port`；`addresses[0]` 恒等于 host:port） */
  addresses: string[];
  /** 访问令牌（拼 iframe URL 用） */
  token?: string;
  /** 系统族（/health 学到） */
  machine_os?: string;
  host?: string;
  port?: number;
  /** 候选走法的优先级表等（后端结构较复杂，前端只透传） */
  [key: string]: unknown;
}

export interface RemoteStoreState {
  servers: RemoteProfile[];
  /** 启动时是否自动连接全部服务端 */
  connect_all: boolean;
  [key: string]: unknown;
}

export const useRemoteStore = defineStore("remote", () => {
  const servers = ref<RemoteProfile[]>([]);
  const connectAll = ref(false);
  /** 每台电脑的引导项信息（`remote_boot_info` 的结果） */
  const bootInfo = ref(new Map<string, RemoteBootInfo>());
  /** 每台电脑的连续失败计数（弱网容忍） */
  const failCounts = ref(new Map<string, number>());
  /** 正在读取的电脑 id */
  const loading = ref(new Set<string>());
  const loaded = ref(false);

  function offlineInfo(error: string, addr = ""): RemoteBootInfo {
    const emptyGroup = (kind: string, title: string) => ({
      kind,
      title,
      available: false,
      current: "",
      next: "",
      saved: "",
      note: error,
      options: [],
    });
    return {
      ok: false,
      online: false,
      addr,
      error,
      refind: emptyGroup("refind", "rEFInd"),
      grub: emptyGroup("grub", "GRUB"),
      channels: [],
    };
  }

  function offlineRetryCount(): number {
    try {
      const raw = localStorage.getItem(OFFLINE_RETRY_KEY);
      if (raw !== null) {
        const n = Number.parseInt(raw, 10);
        if (Number.isFinite(n) && n >= 1) return n;
      }
    } catch {
      /* 吃默认 */
    }
    return OFFLINE_RETRY_DEFAULT;
  }

  /** 清空各机离线失败计数（旧版改 offretry 阈值时 cqFailCount.clear()） */
  function clearFailCounts(): void {
    failCounts.value.clear();
  }

  function reprobeAlways(): boolean {
    try {
      return localStorage.getItem(REPROBE_ALWAYS_KEY) === "1";
    } catch {
      return false;
    }
  }

  /** 读档案列表 */
  const lastError = ref("");
  async function load(): Promise<void> {
    try {
      const store = await call<RemoteStoreState>("remote_list");
      servers.value = store.servers ?? [];
      connectAll.value = Boolean(store.connect_all);
      lastError.value = "";
      loaded.value = true;
    } catch (e) {
      servers.value = [];
      lastError.value = String(e);
      loaded.value = true;
    }
  }

  /**
   * 读一台电脑的引导项（`remote_boot_info`）。
   *
   * **弱网容忍（2026-11-02 用户定案）**：连续失败达到阈值才判离线，
   * 未达阈值时保留上一份数据，卡片不会闪烁。
   */
  async function refreshBoot(id: string): Promise<void> {
    if (loading.value.has(id)) return;
    loading.value.add(id);
    try {
      const info = await call<RemoteBootInfo>("remote_boot_info", { serverId: id });
      if (info.online) {
        failCounts.value.delete(id);
        bootInfo.value.set(id, info);
      } else {
        const fails = (failCounts.value.get(id) ?? 0) + 1;
        failCounts.value.set(id, fails);
        if (fails >= offlineRetryCount()) {
          bootInfo.value.set(id, info); // 达阈值才真正标离线
        }
        // 未达阈值：保留上一份数据，卡片保持在线态
      }
    } catch (error) {
      const fails = (failCounts.value.get(id) ?? 0) + 1;
      failCounts.value.set(id, fails);
      if (fails >= offlineRetryCount()) {
        bootInfo.value.set(id, offlineInfo(String(error)));
      }
    } finally {
      loading.value.delete(id);
    }
  }

  /** 批量刷新（含本机行 `local`） */
  async function refreshAllBoot(): Promise<void> {
    const ids = ["local", ...servers.value.map((s) => s.id)];
    await Promise.all(ids.map((id) => refreshBoot(id)));
  }

  /** 保存档案 */
  async function save(payload: Record<string, unknown>): Promise<void> {
    await call("remote_save", { server: payload });
    await load();
  }

  /** 删除档案 */
  async function remove(id: string): Promise<void> {
    await call("remote_delete", { id });
    bootInfo.value.delete(id);
    failCounts.value.delete(id);
    await load();
  }

  /** 切换引导项（通道 / GRUB / rEFInd 由 kind 区分） */
  async function switchBoot(payload: Record<string, unknown>): Promise<void> {
    await call("remote_boot_switch", payload);
    const id = String(payload.serverId ?? payload.server_id ?? "");
    if (id) await refreshBoot(id);
  }

  /** 电源指令：唤醒 / 重启 / 关机 / 睡眠 */
  async function power(payload: Record<string, unknown>): Promise<void> {
    await call("remote_power", payload);
  }

  /** 测试连接（探活） */
  async function test(payload: Record<string, unknown>): Promise<unknown> {
    return call("remote_test", payload);
  }

  const totalOnline = computed(
    () => [...bootInfo.value.values()].filter((info) => info.online).length,
  );

  return {
    // 数据
    servers,
    connectAll,
    bootInfo,
    failCounts,
    loading,
    loaded,
    lastError,
    totalOnline,
    // 动作
    load,
    refreshBoot,
    refreshAllBoot,
    save,
    remove,
    switchBoot,
    power,
    test,
    reprobeAlways,
    clearFailCounts,
    offlineRetryCount,
  };
});
