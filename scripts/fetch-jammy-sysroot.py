#!/usr/bin/env python3
"""把一个可执行文件的动态依赖闭包，从 Ubuntu 22.04 (jammy) 的软件源里抓成一个 sysroot。

为什么要它
    tauri 的 appimage 打包会把**构建机**的整条 GTK/WebKit 依赖链复制进包（本机 CachyOS
    上是 188 个 .so / 283MB），这些库带着构建机的 glibc 符号版本要求（GLIBC_2.43/2.44），
    换到 glibc 稍老的机器就报 `version 'GLIBC_2.44' not found`。
    Ubuntu 22.04 是**同时满足两个条件的最老基座**：有 Tauri 2 必需的 webkit2gtk-4.1
    （上游 2.50.4，Ubuntu 会持续回移），而它自己的 glibc 是 2.35。
    所以：在 jammy 上取库 → 整包 glibc 门槛从 2.44 降到 2.35。

怎么做（全程不需要特权，也不需要执行任何外来二进制）
    1. 读目标 ELF 的 DT_NEEDED（objdump -p，只是读文件，不运行它）；
    2. 用 jammy 的 Contents 索引把「soname → 哪个包提供」查出来；
    3. 下 .deb（dpkg-deb -x 解包），再对解出来的 .so 递归第 1 步，直到闭包收敛。

    没有容器、没有 chroot、没有 apt —— 本机 CapEff 是空的、ptrace 被 seccomp 挡着，
    容器和 proot 都用不了；这条「只下 deb 解包」的路是唯一可行且不需要特权的。

用法
    python3 scripts/fetch-jammy-sysroot.py --elf src-tauri/target/release/anyboot \
        --dest ~/.cache/ab-sysroot/root

    跑完 dest 下就是一份 jammy 的运行时库树；脚本最后会算出**该裹进 AppImage 的清单**
    （= 收敛后的 soname 集合，剔除 glibc 家族等由目标机提供的），写到 dest/../bundle-list.txt
"""

from __future__ import annotations

import argparse
import gzip
import os
import re
import shutil
import subprocess
import sys
import urllib.request

MIRROR = "https://mirrors.tuna.tsinghua.edu.cn/ubuntu"
SUITES = ("jammy", "jammy-updates", "jammy-security")
COMPS = ("main", "universe")

# 由目标机提供的库，不该裹进包里（跟 linuxdeploy 的默认排除口径一致）。
# 裹了反而危险：glibc 必须和目标机的 ld.so 配套，混搭会出各种诡异问题。
EXCLUDE_RE = re.compile(
    r"^(ld-linux.*|libc\.so.*|libm\.so.*|libdl\.so.*|libpthread\.so.*|librt\.so.*"
    r"|libresolv\.so.*|libnsl\.so.*|libutil\.so.*|libanl\.so.*|libmvec\.so.*"
    r"|libgcc_s\.so.*|libGL\.so.*|libEGL\.so.*|libGLX.*|libGLdispatch.*|libOpenGL.*"
    r"|libdrm\.so.*|libglapi\.so.*|libgbm\.so.*|libvulkan\.so.*|libwayland-.*)$"
)


def log(msg: str) -> None:
    print(f"[sysroot] {msg}", flush=True)


def die(msg: str) -> None:
    print(f"[sysroot] 错误：{msg}", file=sys.stderr, flush=True)
    sys.exit(1)


def fetch(url: str, dest: str) -> None:
    """下载到 dest（已存在且非空就跳过）。"""
    if os.path.exists(dest) and os.path.getsize(dest) > 0:
        return
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    tmp = dest + ".part"
    log(f"下载 {os.path.basename(dest)} …")
    with urllib.request.urlopen(url, timeout=300) as r, open(tmp, "wb") as f:
        shutil.copyfileobj(r, f, length=1 << 20)
    os.replace(tmp, dest)


def needed_sonames(elf: str) -> set[str]:
    """读 ELF 的 DT_NEEDED（只读文件，不执行）。"""
    try:
        out = subprocess.run(["objdump", "-p", elf], capture_output=True, text=True).stdout
    except OSError:
        return set()
    return set(re.findall(r"NEEDED\s+(\S+)", out))


# ---------------------------------------------------------------- Packages 索引

def load_packages(cache: str) -> dict[str, dict]:
    """jammy / -updates / -security 依次覆盖 → 拿到每个包的最新文件名。"""
    idx: dict[str, dict] = {}
    for suite in SUITES:
        for comp in COMPS:
            path = os.path.join(cache, f"Packages-{suite}-{comp}.gz")
            url = f"{MIRROR}/dists/{suite}/{comp}/binary-amd64/Packages.gz"
            fetch(url, path)
            cur = None
            with gzip.open(path, "rt", errors="ignore") as f:
                for line in f:
                    if line.startswith("Package: "):
                        cur = line[9:].rstrip("\n")
                    elif line.startswith("Version: ") and cur:
                        idx.setdefault(cur, {})["version"] = line[9:].rstrip("\n")
                    elif line.startswith("Filename: ") and cur:
                        idx.setdefault(cur, {})["filename"] = line[10:].rstrip("\n")
    log(f"Packages 索引：{len(idx)} 个包")
    return idx


# ---------------------------------------------------------------- Contents 索引

def load_contents(cache: str) -> dict[str, list[str]]:
    """只保留 x86_64-linux-gnu 下的路径 → soname → [包名...]。

    注意 Contents 的包名字段带**分类前缀**（`libs/libgtk-3-0`、
    `universe/libs/libwebkit2gtk-4.1-0`），必须取最后一个 `/` 之后的部分
    才是真包名 —— 不然拿去索引里查会一个都查不到（踩过）。
    """
    path = os.path.join(cache, "Contents-jammy-amd64.gz")
    fetch(f"{MIRROR}/dists/jammy/Contents-amd64.gz", path)
    m: dict[str, list[str]] = {}
    pat = re.compile(r"^(?:usr/)?lib/x86_64-linux-gnu/([^/]+\.so(?:\.[^/]*)?)$")
    with gzip.open(path, "rt", errors="ignore") as f:
        for line in f:
            p = line.rstrip("\n").split(None, 1)
            if len(p) != 2:
                continue
            mm = pat.match(p[0])
            if not mm:
                continue
            names = [x.strip().rsplit("/", 1)[-1] for x in p[1].split(",")]
            names = [n for n in names if n]
            if names:
                m[mm.group(1)] = names
    log(f"Contents 索引：{len(m)} 个 soname")
    return m


def guess_packages(soname: str, idx: dict[str, dict]) -> list[str]:
    """Contents 里查不到时的兜底：按 soname 猜包名（libfoo.so.1 → libfoo1 / libfoo-1 ...）。"""
    base, _, rest = soname.partition(".so")
    ver = rest.lstrip(".")
    out: list[str] = []
    for cand in (f"{base}{ver}", f"{base}-{ver}", base):
        if cand in idx and cand not in out:
            out.append(cand)
    # 再按前缀兜一轮：名字前缀相同、尾巴不超过 6 个字符的（libjxl.so.0.7 → libjxl0.7）
    for name in idx:
        if name.startswith(base) and 0 < len(name) - len(base) <= 6 and name not in out:
            out.append(name)
    return out


# ---------------------------------------------------------------- 主流程

def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--elf", required=True, help="要算依赖闭包的可执行文件/库")
    ap.add_argument("--dest", required=True, help="sysroot 落地目录")
    ap.add_argument("--cache", default=None, help="索引与 deb 缓存（默认 dest 的上一级）")
    ap.add_argument("--extra-soname", action="append", default=[],
                    help="额外强制纳入的 soname（可多次给）")
    args = ap.parse_args()

    if not os.path.exists(args.elf):
        die(f"找不到 {args.elf}")
    dest = os.path.abspath(os.path.expanduser(args.dest))
    cache = os.path.abspath(os.path.expanduser(args.cache)) if args.cache \
        else os.path.dirname(dest)
    os.makedirs(dest, exist_ok=True)
    debdir = os.path.join(cache, "deb")

    idx = load_packages(cache)
    contents = load_contents(cache)

    # 已就位（解出来的）soname → 提供它的文件路径
    def scan_provided() -> dict[str, str]:
        got: dict[str, str] = {}
        for root, _dirs, files in os.walk(dest):
            for fn in files:
                if ".so" in fn:
                    got.setdefault(fn, os.path.join(root, fn))
        return got
    provided = scan_provided()

    pending: set[str] = set(needed_sonames(args.elf)) | set(args.extra_soname)
    pending = {s for s in pending if not EXCLUDE_RE.match(s)}
    log(f"种子 DT_NEEDED：{len(pending)} 个 soname（已剔除 glibc 家族等 {len(needed_sonames(args.elf)) - len(pending)} 个）")

    wanted: set[str] = set()          # 最终要裹的 soname
    unresolved: set[str] = set()
    pkgs_done: set[str] = set()
    downloaded = 0

    while pending:
        soname = sorted(pending)[0]
        pending.discard(soname)
        if soname in provided or soname in wanted:
            wanted.add(soname)
            continue
        cands = contents.get(soname) or guess_packages(soname, idx)
        cands = [c for c in cands if c in idx]
        if not cands:
            unresolved.add(soname)
            continue
        pkg = cands[0]
        wanted.add(soname)
        if pkg in pkgs_done:
            # 包已经下过、也解过了，但没提供这个 soname → 试下一个候选
            for c in cands[1:]:
                if c not in pkgs_done:
                    pkg = c
                    break
            else:
                unresolved.add(soname)
                continue
        pkgs_done.add(pkg)
        deb = os.path.join(debdir, os.path.basename(idx[pkg]["filename"]))
        fetch(f"{MIRROR}/{idx[pkg]['filename']}", deb)
        downloaded += 1
        before = set(provided)
        subprocess.run(["dpkg-deb", "-x", deb, dest], check=True)
        # 只看这轮新解出来的库，别整棵树重扫
        newly = {k: v for k, v in scan_provided().items() if k not in before}
        for fn, full in newly.items():
            provided.setdefault(fn, full)
            for s in needed_sonames(full):
                if not EXCLUDE_RE.match(s) and s not in provided:
                    pending.add(s)
        log(f"  {pkg}（已下 {downloaded} 个包 / 新库 {len(newly)} 个 / 待查 {len(pending)} 个 soname）")

    log("")
    log(f"完成：解包 {len(pkgs_done)} 个包，覆盖 {len(wanted)} 个 soname")
    # 自检：清单里每个 soname 都必须真的在 sysroot 里找到文件
    final = scan_provided()
    absent = sorted(s for s in wanted if s not in final)
    if absent:
        log(f"⚠️ 清单里有 {len(absent)} 个 soname 在 sysroot 里没有对应文件（下的包对不上）：")
        for s in absent[:15]:
            log(f"    {s}")
    if unresolved:
        log(f"⚠️ {len(unresolved)} 个 soname 在 jammy 里找不到提供者：")
        for s in sorted(unresolved):
            log(f"    {s}")
        log("   → 这些要么本机独有（得在 jammy 基座里重建应用），要么要手工补包")
    if not unresolved and not absent:
        log("✅ 闭包收敛，没有落空的 soname")
    listing = os.path.join(cache, "bundle-list.txt")
    with open(listing, "w") as f:
        for s in sorted(wanted):
            f.write(s + "\n")
    log(f"要裹进 AppImage 的清单 → {listing}")


if __name__ == "__main__":
    main()
