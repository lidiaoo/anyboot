#!/usr/bin/env python3
"""从 Rust 源码提取每个 tauri command 的必填参数，生成 cmd_required.json
供严格模式 mock 校验前端 invoke 传参是否完整。"""
import re, pathlib, json, sys
root = pathlib.Path(sys.argv[1] if len(sys.argv)>1 else ".")
rust = ""
for p in (root/"src-tauri/src").rglob("*.rs"):
    rust += p.read_text(encoding="utf-8")+"\n"
sig = {}
for m in re.finditer(r'#\[tauri::command\][^\n]*\n(?:\s*//[^\n]*\n)*\s*pub(?:\(crate\))?\s+(?:async\s+)?fn\s+(\w+)\s*\(([\s\S]*?)\)\s*(?:->|\{)', rust):
    psig = {}
    for pm in re.finditer(r'(\w+)\s*:\s*([^,]+?)(?:,|$)', m.group(2)):
        psig[pm.group(1)] = pm.group(2).strip()
    req = [k for k,v in psig.items()
           if not any(f in v for f in ('State','AppHandle','Window','Webview'))
           and not v.strip().startswith('Option')]
    if req: sig[m.group(1)] = req
out = root/"scripts/ui-compare/cmd_required.json"
out.write_text(json.dumps(sig, ensure_ascii=False, indent=1), encoding="utf-8")
print(f"{len(sig)} commands -> {out}")
