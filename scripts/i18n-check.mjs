#!/usr/bin/env node
/**
 * T1：三端 key 一致性校验
 *   - 校验 ui-vue/src/i18n/messages/*.ts 各语言 key 集合一致（以 zh-CN 为基准）
 *   - 生成/更新 docs/i18n-keys.csv
 * 用法：node scripts/i18n-check.mjs [--write-csv]
 */
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const MSG_DIR = join(process.cwd(), "ui-vue/src/i18n/messages");

/** 用正则把 TS 对象字面量拍平成点分 key（只认我们生成的扁平结构） */
function flattenTs(src) {
  const keys = new Set();
  const stack = [];
  for (const raw of src.split("\n")) {
    const line = raw.trim();
    if (!line || line.startsWith("//")) continue;
    const closeMatch = line.match(/^\},?$/);
    if (closeMatch) { stack.pop(); continue; }
    const objMatch = line.match(/^([A-Za-z0-9_$]+):\s*\{$/);
    if (objMatch) { stack.push(objMatch[1]); continue; }
    const kvMatch = line.match(/^([A-Za-z0-9_$]+):\s*"((?:[^"\\]|\\.)*)",?$/);
    if (kvMatch) { keys.add([...stack, kvMatch[1]].join(".")); continue; }
  }
  return keys;
}

const files = readdirSync(MSG_DIR).filter((f) => f.endsWith(".ts"));
const base = "zh-CN.ts";
if (!files.includes(base)) { console.error("缺少 zh-CN.ts"); process.exit(1); }
const baseKeys = flattenTs(readFileSync(join(MSG_DIR, base), "utf8"));
let bad = 0;
for (const f of files) {
  if (f === base) continue;
  const keys = flattenTs(readFileSync(join(MSG_DIR, f), "utf8"));
  const missing = [...baseKeys].filter((k) => !keys.has(k));
  const extra = [...keys].filter((k) => !baseKeys.has(k));
  if (missing.length || extra.length) {
    bad++;
    console.error(`✗ ${f}: 缺 ${missing.length} 条，多 ${extra.length} 条`);
    if (missing.length) console.error("   缺:", missing.slice(0, 10).join(", "));
    if (extra.length) console.error("   多:", extra.slice(0, 10).join(", "));
  }
}
if (bad) { console.error(`\n✗ ${bad} 个语言包 key 与 ${base} 不一致`); process.exit(1); }

if (process.argv.includes("--write-csv")) {
  const rows = ["key,zh-CN,en-US"];
  for (const k of [...baseKeys].sort()) rows.push(`${k},,""`);
  writeFileSync(join(process.cwd(), "docs/i18n-keys.csv"), rows.join("\n") + "\n", "utf8");
  console.log("✓ 已写入 docs/i18n-keys.csv");
}
console.log(`✓ i18n-check：${files.length} 个语言包，${baseKeys.size} 条 key，全部一致`);
