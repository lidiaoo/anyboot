/** 引导器的展示元数据（与 `ui/boot-adv.js:25` 的 LOADER_META 保持一致） */

export interface LoaderMeta {
  name: string;
  /** 图标里的两个字母 */
  mono: string;
  color: string;
  /** file = 免提权；nvram = 切换必经 NVRAM 写 */
  perm: "file" | "nvram";
}

export const LOADER_META: Record<string, LoaderMeta> = {
  refind: { name: "rEFInd", mono: "rE", color: "#14b8a6", perm: "file" },
  // RefindPlus：与 rEFInd 并列的独立通道（id 参照后端 channel 名）
  refindplus: { name: "RefindPlus", mono: "RP", color: "#f59e0b", perm: "file" },
  grub: { name: "GRUB", mono: "GR", color: "#f59e0b", perm: "file" },
  sdboot: { name: "systemd-boot", mono: "sd", color: "#38bdf8", perm: "file" },
  opencore: { name: "OpenCore", mono: "OC", color: "#6366f1", perm: "nvram" },
  clover: { name: "Clover", mono: "CL", color: "#a78bfa", perm: "file" },
};

export function loaderMeta(id: string): LoaderMeta {
  return LOADER_META[id] ?? { name: id, mono: id.slice(0, 2), color: "#888", perm: "file" };
}
