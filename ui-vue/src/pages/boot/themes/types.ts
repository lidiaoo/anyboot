/** 主题渲染器共用的类型 */

/** 一个引导器在主题里的展示信息 */
export interface LoaderInfo {
  /** refind / grub / sdboot / opencore / clover */
  id: string;
  /** 显示名（rEFInd / GRUB / systemd-boot …） */
  name: string;
  /** 状态：ok 已检测免提权 / warn 需提权 / off 未检测到 */
  state: "ok" | "warn" | "off";
  /** 条目数（快捷条上的角标） */
  entries: unknown[];
  /** ESP 描述（副标题用） */
  esp?: string;
  /** file / nvram */
  perm?: string;
  detected?: boolean;
}
