/** 引导列表的公共类型（从 .vue 里抽出来，方便 TS 直接 import） */

export interface EntryData {
  /** GRUB selector；通道 / OC 用标题兜底 */
  selector?: string;
  name: string;
  /** 图标分类：mac / win / linux / auto */
  class?: string;
  kernels?: string[];
  variants?: number;
  chainloader?: boolean;
  submenu?: string | null;
  /** 旧版 refindSim 子行右侧来源标签（默认项 / refind_linux.conf / …） */
  tag?: string;
}

export interface GroupInfo {
  /** submenu id */
  id: string;
  title: string;
}
