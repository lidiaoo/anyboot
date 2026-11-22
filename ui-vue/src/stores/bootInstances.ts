import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { call, isTauri } from "@/api/tauri";
import { useBootAdvStore } from "@/stores/bootAdv";

/** 多实例引导器的可见性 / 顺序 / 左右列。统一存后端 bootAdvJson（跨桌面/web），不再只用 localStorage。 */
export const useBootInstancesStore = defineStore("bootInstances", () => {
  const adv = useBootAdvStore();
  /** 唯一实例数据源：本应用内只请求一次，BootApp/BootSettings/BootList 共用。 */
  const list = ref<any[]>([]);
  const loading = ref(false);
  let pending: Promise<any[]> | null = null;

  const state = computed(() => ({
    order: adv.state.instanceOrder,
    hidden: adv.state.instanceHidden,
    sides: adv.state.instanceSides,
  }));

  async function load(force = false): Promise<any[]> {
    if (!force && list.value.length) return list.value;
    if (pending && !force) return pending; // force 必须另开一次，等旧 pending 会拿到扫描前快照
    loading.value = true;
    pending = (async () => {
      let data: any[] = [];
      // 桌面端优先用本机 HTTP 直取（已加 CORS，绕开 IPC/ACL 的不确定性）。
      if (isTauri()) {
        try {
          const endpoint: any = await call<any>("local_http_endpoint");
          if (endpoint?.port) {
            const response = await fetch(`http://127.0.0.1:${endpoint.port}/api/boot-channels?token=${encodeURIComponent(endpoint.token || "")}`);
            const list = await response.json();
            if (Array.isArray(list)) data = list;
          }
        } catch { /* 继续 */ }
      }
      // 兼容老命令 remote_boot_info('local')：其 channels 就是实例列表。
      try {
        const info: any = await call<any>("remote_boot_info", { serverId: "local" });
        if (Array.isArray(info?.channels)) data = info.channels;
      } catch { /* 继续 */ }
      if (!data.length) {
        try {
          const result: any = await call<any[]>("boot_channels_all");
          if (Array.isArray(result)) data = result;
        } catch { /* 继续 */ }
      }
      if (!data.length) {
        try {
          const token = new URLSearchParams(location.search).get("token") || "";
          const response = await fetch(`/api/boot-channels?token=${encodeURIComponent(token)}`);
          const result = await response.json();
          if (Array.isArray(result)) data = result;
        } catch { /* 忽略 */ }
      }
      if (data.length) {
        list.value = data;
        setInstances(data);
      }
      loading.value = false;
      return list.value;
    })();
    try { return await pending; } finally { pending = null; }
  }

  function idOf(item: any): string {
    return String(item?.instanceId || item?.kind || "");
  }

  /** 新实例列表到达：补进 order 末尾、清理已消失的 id；保留后端已 hydrate 的顺序。 */
  function setInstances(items: any[]) {
    const ids = items.map(idOf).filter(Boolean);
    // **只补不删**：扫描不全（挂载未就绪 / 临时错误）时若把 order 里的 id 剪掉，
    // 会把页签永久删掉（用户实测 GRUB 页签消失）。删除交给人手在「引导高级设置」里做。
    const order = [...adv.state.instanceOrder];
    for (const id of ids) if (!order.includes(id)) order.push(id);
    const hidden = adv.state.instanceHidden.filter((id) => ids.includes(id) || !ids.length);
    const sides = { ...adv.state.instanceSides };
    if (order.join("\u0000") !== adv.state.instanceOrder.join("\u0000")
      || hidden.join("\u0000") !== adv.state.instanceHidden.join("\u0000")
      || JSON.stringify(sides) !== JSON.stringify(adv.state.instanceSides)) {
      adv.setInstanceAll(order, hidden, sides);
    }
  }

  /** 切换成功后就地改 current（本机 1.5s / 远端 7s 的全量刷新放后台） */
  function patchCurrent(id: string, current: string): void {
    const item = list.value.find((x: any) => idOf(x) === id);
    if (item) {
      item.current = current;
      list.value = [...list.value];
    }
  }

  function isVisible(id: string): boolean {
    return !adv.state.instanceHidden.includes(id);
  }

  function toggleVisible(id: string) {
    const hidden = adv.state.instanceHidden.includes(id)
      ? adv.state.instanceHidden.filter((x) => x !== id)
      : [...adv.state.instanceHidden, id];
    adv.setInstanceAll([...adv.state.instanceOrder], hidden, { ...adv.state.instanceSides });
  }

  function move(id: string, delta: number) {
    const order = [...adv.state.instanceOrder];
    const i = order.indexOf(id);
    const j = i + delta;
    if (i < 0 || j < 0 || j >= order.length) return;
    order.splice(j, 0, ...order.splice(i, 1));
    adv.setInstanceAll(order, [...adv.state.instanceHidden], { ...adv.state.instanceSides });
  }

  function sideOf(id: string): string {
    return adv.state.instanceSides[id] === "r" ? "r" : "l";
  }

  function toggleSide(id: string) {
    const sides = { ...adv.state.instanceSides, [id]: sideOf(id) === "r" ? "l" : "r" };
    adv.setInstanceAll([...adv.state.instanceOrder], [...adv.state.instanceHidden], sides);
  }

  function reset(items: any[]) {
    adv.setInstanceAll(items.map(idOf).filter(Boolean), [], {});
  }

  /** 实例数组按用户排序 + 可见性过滤。 */
  function orderedVisible(items: any[]): any[] {
    const map = new Map(items.map((item) => [idOf(item), item]));
    return adv.state.instanceOrder
      .map((id) => map.get(id))
      .filter((item): item is any => !!item)
      .filter((item) => isVisible(idOf(item)));
  }

  return { state, list, loading, load, setInstances, patchCurrent, isVisible, toggleVisible, move, sideOf, toggleSide, reset, orderedVisible, idOf };
});
