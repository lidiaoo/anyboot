#!/usr/bin/env node
/**
 * T0：中文残留扫描（i18n 覆盖率闸门）
 *   - 扫 ui-vue/src（PC）、mobile/android（Kotlin）、mobile/ios（Swift）里的中文字面量
 *   - 跳过：注释、i18n 语言包目录、白名单文件、品牌词行
 * 用法：node scripts/i18n-scan.mjs [pc|android|ios|all] [--limit N]
 * 退出码：0 = 干净；1 = 有残留（CI 用）
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, extname, relative } from "node:path";

const ROOT = process.cwd();
const CJK = /[\u3400-\u9fff\uf900-\ufaff]/;

// 白名单：文件路径（相对仓库根）整文件跳过
const WHITELIST_FILES = [
  "ui-vue/src/i18n/messages/",
  "ui-vue/src/pages/boot/themes/", // 主题演示数据（后续批次处理）
  "mobile/ios/AnyBootRemote/DemoSeed.swift",
  "mobile/android/app/src/main/java/com/anyboot/remote/DemoSeed.kt",
];
// 白名单：行内出现这些词则放过（品牌/技术词/日志）
const WHITELIST_WORDS = ["GRUB", "rEFInd", "RefindPlus", "OpenCore", "systemd-boot", "Clover",
  "ESP", "NVRAM", "APFS", "OCBtOrder", "OCBt", "trace(", "console.", "Log.", "print(", "#Preview"];

function walk(dir, exts, out = []) {
  let entries;
  try { entries = readdirSync(dir); } catch { return out; }
  for (const name of entries) {
    if (name === "node_modules" || name === "dist" || name.startsWith(".")) continue;
    const full = join(dir, name);
    const st = statSync(full);
    if (st.isDirectory()) walk(full, exts, out);
    else if (exts.includes(extname(name))) out.push(full);
  }
  return out;
}

function scan(files) {
  const hits = [];
  for (const f of files) {
    const rel = relative(ROOT, f).replaceAll("\\", "/");
    if (WHITELIST_FILES.some((w) => rel.startsWith(w))) continue;
    const lines = readFileSync(f, "utf8").split("\n");
    lines.forEach((line, i) => {
      const s = line.trim();
      if (!CJK.test(line)) return;
      if (s.startsWith("//") || s.startsWith("*") || s.startsWith("/*") || s.startsWith("<!--")) return;
      // 去掉注释尾部：只检查注释前的代码部分
      const code = line.replace(/\/\/.*$/, "").replace(/\/\*.*?\*\//g, "");
      if (!CJK.test(code)) return;
      if (WHITELIST_WORDS.some((w) => line.includes(w))) return;
      // 白名单：字符串里只有品牌/数字/符号（无中文）不会到这里；此处命中说明含中文
      hits.push({ file: rel, line: i + 1, text: s.slice(0, 120) });
    });
  }
  return hits;
}

const target = process.argv[2] || "all";
const limitArg = process.argv.indexOf("--limit");
const limit = limitArg > 0 ? Number(process.argv[limitArg + 1]) : 200;

const sets = {
  pc: () => walk(join(ROOT, "ui-vue/src"), [".vue", ".ts"]),
  android: () => walk(join(ROOT, "mobile/android/app/src/main/java/com/anyboot/remote"), [".kt"]),
  ios: () => walk(join(ROOT, "mobile/ios/AnyBootRemote"), [".swift"]),
};
const files = target === "all" ? [...sets.pc(), ...sets.android(), ...sets.ios()] : sets[target]();
const hits = scan(files);

if (!hits.length) {
  console.log(`✓ i18n-scan(${target})：无中文残留`);
  process.exit(0);
}
const byFile = new Map();
for (const h of hits) byFile.set(h.file, (byFile.get(h.file) || 0) + 1);
console.log(`✗ i18n-scan(${target})：${hits.length} 处中文残留，涉及 ${byFile.size} 个文件（前 ${limit} 条）：`);
for (const h of hits.slice(0, limit)) console.log(`  ${h.file}:${h.line}  ${h.text}`);
console.log("\n按文件统计（Top 30）：");
[...byFile.entries()].sort((a, b) => b[1] - a[1]).slice(0, 30)
  .forEach(([f, n]) => console.log(`  ${n.toString().padStart(4)}  ${f}`));
process.exit(1);
