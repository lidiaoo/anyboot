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
  /** 每台服务器「测试连接」的结果文字（serverId → '通 · N ms' / '不通：原因'）。
   *  ServersPage 的连通性/延迟/延迟趋势都从这份读；之前 store 里根本没有这个 Map →
   *  点「测试连接」无任何反馈。 */
  const testResults = ref(new Map<string, string>());
  /** 每台电脑各连接通道的探测明细（remote_test_via 的 results；电脑列表逐通道连通性展示用） */
  const probeResults = ref(new Map<string, { kind: string; address: string; ok: boolean; ms: number; error: string; pair_id?: string; remote?: boolean }[]>());
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
    // 【修复】之前直接把整个 HubServer 对象传给 remote_test，但该命令要扁平参数
    // (addresses/probe_mode/...)，对不上 → 测试无效、始终「不通」。
    // 改用 remote_test_via：只传 serverId，后端自己查 profile 逐通道探测（与编辑对话框一致）。
    const id = String(payload.id ?? payload.serverId ?? "");
    let text = "不通";
    try {
      const r = await call<{ ok?: boolean; ms?: number; addr?: string; error?: string }>(
        "remote_test_via",
        { serverId: id, address: null },
      );
      text = r?.ok ? `通 · ${r.ms ?? 0} ms` : `不通：${r?.error ?? ""}`;
    } catch (e) {
      text = `不通：${e}`;
    }
    if (id) {
      testResults.value.set(id, text);
      testResults.value = new Map(testResults.value);
    }
    return text;
  }

  /** 测试全部服务器连接（ServersPage 顶部「刷新」按钮） */
  async function testAll(): Promise<void> {
    await Promise.all((servers.value ?? []).map((s) => test({ id: s.id }).catch(() => {})));
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
    testResults,
    probeResults,
    // 动作
    testAll,
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
