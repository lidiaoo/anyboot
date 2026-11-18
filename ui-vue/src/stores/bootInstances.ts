import { defineStore } from "pinia";
import { ref } from "vue";

/** 多实例引导器的可见性 / 顺序 / 左右列，PC「系统引导」与「引导高级设置」共用并持久化。 */
const KEY = "anyboot.bootInstances";

interface Prefs {
  order: string[];
  hidden: string[];
  sides: Record<string, string>;
}

function load(): Prefs {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<Prefs>;
      return {
        order: Array.isArray(parsed.order) ? parsed.order : [],
        hidden: Array.isArray(parsed.hidden) ? parsed.hidden : [],
        sides: parsed.sides && typeof parsed.sides === "object" ? parsed.sides : {},
      };
    }
  } catch { /* 忽略 */ }
  return { order: [], hidden: [], sides: {} };
}

export const useBootInstancesStore = defineStore("bootInstances", () => {
  const state = ref<Prefs>(load());

  function save() {
    try { localStorage.setItem(KEY, JSON.stringify(state.value)); } catch { /* 忽略 */ }
  }

  function idOf(item: any): string {
    return String(item?.instanceId || item?.kind || "");
  }

  /** 新实例列表到达：补进 order 末尾、清理已消失的 id；首次进入按后端顺序。 */
  function setInstances(list: any[]) {
    const ids = list.map(idOf).filter(Boolean);
    const order = state.value.order.filter((id) => ids.includes(id));
    for (const id of ids) if (!order.includes(id)) order.push(id);
    state.value = {
      order,
      hidden: state.value.hidden.filter((id) => ids.includes(id)),
      sides: Object.fromEntries(Object.entries(state.value.sides).filter(([id]) => ids.includes(id))),
    };
    save();
  }

  function isVisible(id: string): boolean {
    return !state.value.hidden.includes(id);
  }

  function toggleVisible(id: string) {
    state.value.hidden = state.value.hidden.includes(id)
      ? state.value.hidden.filter((x) => x !== id)
      : [...state.value.hidden, id];
    save();
  }

  function move(id: string, delta: number) {
    const order = [...state.value.order];
    const i = order.indexOf(id);
    const j = i + delta;
    if (i < 0 || j < 0 || j >= order.length) return;
    order.splice(j, 0, ...order.splice(i, 1));
    state.value.order = order;
    save();
  }

  function sideOf(id: string): string {
    return state.value.sides[id] === "r" ? "r" : "l";
  }

  function toggleSide(id: string) {
    state.value.sides = { ...state.value.sides, [id]: sideOf(id) === "r" ? "l" : "r" };
    save();
  }

  function reset(list: any[]) {
    state.value = { order: list.map(idOf).filter(Boolean), hidden: [], sides: {} };
    save();
  }

  /** 实例数组按用户排序 + 可见性过滤。 */
  function orderedVisible(list: any[]): any[] {
    const map = new Map(list.map((item) => [idOf(item), item]));
    return state.value.order
      .map((id) => map.get(id))
      .filter((item): item is any => !!item)
      .filter((item) => isVisible(idOf(item)));
  }

  return { state, setInstances, isVisible, toggleVisible, move, sideOf, toggleSide, reset, orderedVisible, idOf };
});
