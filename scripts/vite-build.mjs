#!/usr/bin/env node
/**
 * vite 构建包装器（2026-11-19）
 *
 * 背景：Windows 上偶尔会看到
 *   [vite:esbuild-transpile] remove C:\Users\…\Temp\esbuild-<hash>: Access is denied.
 * 构建产物其实已经全部生成（「✓ 194 modules transformed」之后才炸），失败点在
 * **esbuild 收工时删自己的临时文件**：那份 1~2MB 的临时文件刚写完就被 Defender /
 * 索引器 / 网盘同步抓着扫，Windows 上删除返回 ERROR_ACCESS_DENIED，esbuild 直接
 * 把它当构建错误抛出。过几秒同样的命令又能过——典型的时间窗竞争，不是代码问题。
 *
 * 这里做两件事：
 *   1. 把 TMP/TEMP 固定到项目内 `.build/tmp`（和 build:win 一直以来的做法一致），
 *      不跟系统 Temp 的清理策略/安全软件扫描路径抢；
 *   2. 只在「错误文本命中临时文件类 EPERM/EBUSY」时自动重试，最多 3 次；
 *      真正的编译错误（语法、类型、解析）不重试，原样退出。
 *
 * 用法：node scripts/vite-build.mjs [vite 的额外参数…]
 */
import { spawn } from "node:child_process";
import { mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const tmpDir = path.join(root, ".build", "tmp");
mkdirSync(tmpDir, { recursive: true });

const isWin = process.platform === "win32";
const viteBin = path.join(root, "node_modules", ".bin", isWin ? "vite.cmd" : "vite");
const args = ["build", "--config", "ui-vue/vite.config.ts", ...process.argv.slice(2)];
const MAX_ATTEMPTS = 3;

/** 上一次构建残留的 esbuild 临时文件（锁已释放的话顺手清掉，避免 .build/tmp 越堆越大） */
function sweepStaleTemp() {
  let names = [];
  try {
    names = readdirSync(tmpDir);
  } catch {
    return;
  }
  const now = Date.now();
  for (const name of names) {
    if (!name.startsWith("esbuild-")) continue;
    const full = path.join(tmpDir, name);
    try {
      // 只清 10 分钟以前的：本次构建正在用的那份不能被我们删掉
      if (now - statSync(full).mtimeMs < 10 * 60 * 1000) continue;
      rmSync(full, { force: true, recursive: true });
    } catch {
      /* 还被占着就留着，下次再说 */
    }
  }
}

function runOnce(onStderr) {
  return new Promise((resolve) => {
    const child = spawn(viteBin, args, {
      cwd: root,
      shell: isWin, // vite.cmd 需要经过 cmd.exe
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, TMP: tmpDir, TEMP: tmpDir },
    });
    child.stdout.on("data", (chunk) => process.stdout.write(chunk));
    child.stderr.on("data", (chunk) => {
      onStderr(String(chunk));
      process.stderr.write(chunk);
    });
    child.on("error", (err) => {
      process.stderr.write(`\n[vite-build] 启动失败：${err?.message ?? err}\n`);
      resolve(1);
    });
    child.on("exit", (code) => resolve(code ?? 1));
  });
}

/** 只有「临时文件被占用」这一类才值得重试 */
const FLAKY = /Access is denied|EPERM|EBUSY|ETXTBSY|resource busy/i;

let lastCode = 1;
for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
  sweepStaleTemp();
  let stderrText = "";
  lastCode = await runOnce((text) => {
    stderrText += text;
  });
  if (lastCode === 0) process.exit(0);
  if (attempt < MAX_ATTEMPTS && FLAKY.test(stderrText)) {
    process.stderr.write(
      `\n[vite-build] 第 ${attempt} 次构建被临时文件占用打断（esbuild 清理时 Access denied），` +
        `重试第 ${attempt + 1} 次…\n`,
    );
    continue;
  }
  break;
}

process.exit(lastCode);
