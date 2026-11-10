#!/usr/bin/env node
// 统一 Python 入口。
//
// Windows 上 `python3` 经常被 Microsoft Store 的 AppInstallerPythonRedirector.exe
// 占坑（%LOCALAPPDATA%\Microsoft\WindowsApps\python3.exe）：它既不执行脚本，
// 退出码还非 0。直接写死 python3 会让 `vite build && python3 gen-ui-assets.py`
// 静默断在第二步，后面的 tauri build 根本不会跑。
//
// 这里按顺序探测解释器，跑一段最小代码确认「拿到的真的是 CPython」，
// 取第一个能用的来执行目标脚本。非 Windows 平台首选 python3，行为与原来一致。
//
// 用法：node scripts/run-python.js scripts/gen-ui-assets.py [args...]

const { spawnSync } = require("node:child_process");

const CANDIDATES =
  process.platform === "win32"
    ? [
        ["python3", []],
        ["python", []],
        ["py", ["-3"]],
      ]
    : [
        ["python3", []],
        ["python", []],
      ];

const args = process.argv.slice(2);
if (args.length === 0) {
  console.error("用法: node scripts/run-python.js <script.py> [args...]");
  process.exit(2);
}

/** 商店重定向器/unix 壳脚本的特征：报错里出现这些字样，或 sys.executable 落在 WindowsApps。 */
function looksLikeRedirector(text) {
  return /WindowsApps|AppInstallerPythonRedirector/i.test(text);
}

function usable(cmd, extra) {
  const probe = spawnSync(
    cmd,
    [...extra, "-c", "import sys; print(sys.executable)"],
    { encoding: "utf8", timeout: 15000, windowsHide: true }
  );
  if (probe.error || probe.status !== 0) return false;
  const exe = (probe.stdout || "").trim();
  if (!exe) return false;
  if (looksLikeRedirector(exe) || looksLikeRedirector(probe.stderr || "")) return false;
  return true;
}

const picked = CANDIDATES.find(([cmd, extra]) => usable(cmd, extra));

if (!picked) {
  const names = CANDIDATES.map(([cmd, extra]) => [cmd, ...extra].join(" ")).join(" / ");
  console.error(`[run-python] 没找到可用的 Python 解释器（试过：${names}）`);
  console.error("[run-python] 提示：确认已安装 Python，或把它的目录加入 PATH 并重开终端。");
  process.exit(127);
}

const [cmd, extra] = picked;
const run = spawnSync(cmd, [...extra, ...args], { stdio: "inherit" });
if (run.error) {
  console.error(`[run-python] 执行失败：${run.error.message}`);
  process.exit(1);
}
process.exit(run.status === null ? 1 : run.status);
