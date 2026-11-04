/** 主题渲染器共用的类型 */

/** 一个引导器在主题里的展示信息 */
export interface LoaderInfo {
  /** refind / grub / sdboot / opencore / clover */
  id: string;
  /** 显示名（rEFInd / GRUB / systemd-boot …） */
  name: string;
  /** 状态：ok 已检测免提权 / warn 需提权 / off 未检测到 */
  state: "ok" | "warn" | "off";
  /** 条目（快捷条角标 = length；设置面板用 t/def 取默认项） */
  entries: { t?: string; def?: boolean; [k: string]: unknown }[];
  /** ESP 描述（副标题用） */
  esp?: string;
  /** file / nvram */
  perm?: string;
  detected?: boolean;
  /** 默认项（旧版 nextBoot：entries.find(def)）——设置面板「默认项」显示它 */
  nextBoot?: { t?: string } | null;
  /** 健康/说明文案（旧版 healthText） */
  healthText?: string;
}
