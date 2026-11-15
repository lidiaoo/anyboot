#!/usr/bin/env node
/** 校验代码里引用的 i18n key 都存在于 zh-CN.ts（防「显示 key 原文」） */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, extname, relative } from "node:path";
const ROOT = process.cwd();
const MSG = join(ROOT, "ui-vue/src/i18n/messages/zh-CN.ts");
function flatten(src){const keys=new Set();const stack=[];for(const raw of src.split("\n")){const l=raw.trim();if(!l||l.startsWith("//"))continue;if(/^\},?$/.test(l)){stack.pop();continue;}const o=l.match(/^([A-Za-z0-9_$]+):\s*\{$/);if(o){stack.push(o[1]);continue;}const kv=l.match(/^([A-Za-z0-9_$]+):\s*"((?:[^"\\]|\\.)*)",?$/);if(kv){keys.add([...stack,kv[1]].join("."));continue;}}return keys;}
const known = flatten(readFileSync(MSG,"utf8"));
function walk(dir,out=[]){let es;try{es=readdirSync(dir);}catch{return out;}for(const n of es){if(n==="node_modules"||n==="dist"||n.startsWith("."))continue;const f=join(dir,n);const st=statSync(f);if(st.isDirectory())walk(f,out);else if([".vue",".ts"].includes(extname(n)))out.push(f);}return out;}
const files = walk(join(ROOT,"ui-vue/src")).filter(f=>!f.includes("/i18n/messages/"));
const missing = new Map();
for(const f of files){
  const src = readFileSync(f,"utf8");
  const re = /(?<![\w$])(?:\$t|t)\(\s*["']([A-Za-z0-9_.]+)["']/g;
  let m; while((m=re.exec(src))){
    const k=m[1];
    if(k.endsWith(".")||k.startsWith(".")) continue;
    if(!known.has(k)){ if(!missing.has(k)) missing.set(k,[]); missing.get(k).push(relative(ROOT,f)); }
  }
}
if(!missing.size){console.log(`✓ i18n-refs：代码引用的 key 全部存在（${known.size} 条）`);process.exit(0);}
console.log(`✗ i18n-refs：${missing.size} 个 key 在代码中引用但词典缺失：`);
for(const [k,fs] of missing) console.log(`  ${k}  ← ${[...new Set(fs)].slice(0,3).join(", ")}`);
process.exit(1);
