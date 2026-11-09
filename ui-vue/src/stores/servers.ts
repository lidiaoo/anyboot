/**
 * SSH / SFTP 服务器档案（`hub_*` 命令族）。
 *
 * 与「电脑」档案（`remote_*`）是**两套完全独立的**东西：
 *   · `hub`    → SSH/SFTP 服务器（终端、文件、隧道用）
 *   · `remote` → AnyBoot 电脑（电脑列表、电脑页、引导项用）
 */

import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { call } from "@/api/tauri";

/** 对应 Rust `hub::HubServer` */
export interface HubServer {
  id: string;
  name: string;
  /** "ssh" | "telnet" */
  protocol: string;
  host: string;
  port: number;
  /** 全部访问地址（`host:port`；`addresses[0]` 恒等于 host:port） */
  addresses: string[];
  username: string;
  notes: string;
  /** 密钥文件基名；空 = 不用密钥 */
  key_id: string;
  /** AES-256-GCM 密文；空 = 未存密码 */
  password_enc: string;
  /** 经由哪台跳板机（另一台 server 的 id）；空 = 直连 */
  jump_id: string;
  /** 终端启动路径（空 = 远端家目录） */
  term_path: string;
  /** 终端 shell（空 = 登录 shell） */
  term_shell: string;
  created_at: number;
}

export type Protocol = "ssh" | "telnet";

export const useServersStore = defineStore("servers", () => {
  const servers = ref<HubServer[]>([]);
  const loaded = ref(false);
  /** 正在测试连接的 id */
  const testing = ref(new Set<string>());
  /** 每台的测试结果摘要（id → 文字） */
  const testResults = ref(new Map<string, string>());

  async function load(): Promise<void> {
    try {
      servers.value = await call<HubServer[]>("hub_list_servers");
    } catch {
      servers.value = [];
    } finally {
      loaded.value = true;
    }
  }

  /** 按协议补默认端口（0 = 用户没填） */
  function effectivePort(server: HubServer): number {
    if (server.port !== 0) return server.port;
    return server.protocol === "telnet" ? 23 : 22;
  }

  /** 显示用的地址 */
  function displayAddr(server: HubServer): string {
    // 旧版 hostLabel：username@host:port +「 +N」多地址后缀
    const extra =
      server.addresses && server.addresses.length > 1 ? ` +${server.addresses.length - 1}` : "";
    return `${server.username ? server.username + "@" : ""}${server.host}:${effectivePort(server)}${extra}`;
  }

  async function save(payload: Record<string, unknown>): Promise<void> {
    await call("hub_save_server", { server: payload });
    await load();
  }

  async function remove(id: string): Promise<void> {
    await call("hub_delete_server", { id });
    servers.value = servers.value.filter((item) => item.id !== id);
  }

  /** 本地测得的延迟（ms），与 v0.2.5 probe() 同口径（用本地耗时采样）。 */
  const testMs = ref(new Map<string, number>());

  async function test(server: HubServer): Promise<void> {
    if (testing.value.has(server.id)) return;
    testing.value.add(server.id);
    const t0 = performance.now();
    try {
      // hub_test_server 返回 Result<String,String>：成功=消息字符串(如"已连通 host:port")，
      // 失败=抛错(消息)。之前误当成 {ok,ms,message} 对象解析 → result.ok 恒 undefined → 永远显示「不通」。
      const message = await call<string>("hub_test_server", { server });
      const ms = Math.max(1, Math.round(performance.now() - t0));
      testMs.value.set(server.id, ms);
      testMs.value = new Map(testMs.value);
      // 通 · N ms · 后端消息（后端可能已含 host:port）
      const detail = String(message ?? "").trim();
      testResults.value.set(server.id, `通 · ${ms} ms${detail ? " · " + detail : ""}`);
    } catch (error) {
      testMs.value.delete(server.id);
      testMs.value = new Map(testMs.value);
      testResults.value.set(server.id, String(error));
    } finally {
      testing.value.delete(server.id);
    }
  }

  async function testAll(): Promise<void> {
    await Promise.all(servers.value.map((server) => test(server)));
  }

  const byProtocol = computed(() => ({
    ssh: servers.value.filter((s) => s.protocol === "ssh").length,
    telnet: servers.value.filter((s) => s.protocol === "telnet").length,
    all: servers.value.length,
  }));

  return {
    servers,
    loaded,
    testing,
    testResults,
    testMs,
    byProtocol,
    load,
    save,
    remove,
    test,
    testAll,
    effectivePort,
    displayAddr,
  };
});
