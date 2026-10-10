"""AnyBoot：托盘右键菜单「压任务栏」问题的取证脚本（Windows，只用标准库）。

为什么要留这个脚本：`popup_tray_menu()` 的核心结论是「Windows 的 TrackPopupMenu 只保证菜单
不越出**屏幕**、不保证不越出**工作区**，所以必须换锚点」——这是个平台行为断言，光靠注释
没法复核。跑一遍这个脚本，就能在任意一台 Windows 上量到菜单窗口（类名 `#32768`）的真实
rect，以及它相对工作区下沿越界了多少像素。

用法（任意 Python 3，无需第三方包）：

    python win32-menu-geometry.py

会依次弹三次菜单（每次约 0.5 秒，自己按 ESC 收掉），打印各自 rect：
  1. tray-icon 原样：TPM_BOTTOMALIGN|TPM_LEFTALIGN，锚点 = 托盘图标上的光标
  2. muda 原样：TPM_LEFTALIGN，同一锚点
  3. AnyBoot 的做法：TPM_BOTTOMALIGN|TPM_LEFTALIGN，锚点 = 工作区下沿

于是 1/2 会「压住任务栏」，3 是 0px 越界。

坑（踩过）：**建窗口、建菜单、TrackPopupMenu 必须在同一个线程里**。跨线程建窗口再弹出，
TrackPopupMenu 会直接返回 0（`ERROR_INVALID_WINDOW_HANDLE` / 1400），什么也量不到。
"""
import ctypes
import ctypes.wintypes as w
import threading
import time

u32 = ctypes.WinDLL("user32", use_last_error=True)

SPI_GETWORKAREA = 0x0030
SM_CXSCREEN, SM_CYSCREEN = 0, 1
WM_KEYDOWN, VK_ESCAPE = 0x0100, 0x1B
TPM_LEFTALIGN, TPM_BOTTOMALIGN = 0x0000, 0x0020
WS_POPUP, WS_VISIBLE = 0x80000000, 0x10000000


class RECT(ctypes.Structure):
    _fields_ = [("left", w.LONG), ("top", w.LONG), ("right", w.LONG), ("bottom", w.LONG)]


u32.SystemParametersInfoW.argtypes = [w.UINT, w.UINT, ctypes.c_void_p, w.UINT]
u32.GetSystemMetrics.argtypes = [ctypes.c_int]
u32.CreateWindowExW.restype = w.HWND
u32.CreateWindowExW.argtypes = [
    w.DWORD, w.LPCWSTR, w.LPCWSTR, w.DWORD, ctypes.c_int, ctypes.c_int,
    ctypes.c_int, ctypes.c_int, w.HWND, w.HMENU, w.HINSTANCE, w.LPVOID,
]
u32.CreatePopupMenu.restype = w.HMENU
u32.AppendMenuW.restype = w.BOOL
u32.AppendMenuW.argtypes = [w.HMENU, w.UINT, ctypes.c_size_t, w.LPCWSTR]
u32.TrackPopupMenu.restype = w.BOOL
u32.TrackPopupMenu.argtypes = [
    w.HMENU, w.UINT, ctypes.c_int, ctypes.c_int, ctypes.c_int, w.HWND, ctypes.c_void_p,
]
u32.GetWindowRect.argtypes = [w.HWND, ctypes.POINTER(RECT)]
u32.FindWindowW.restype = w.HWND
u32.FindWindowW.argtypes = [w.LPCWSTR, w.LPCWSTR]
u32.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]


def work_area():
    rect = RECT()
    u32.SystemParametersInfoW(SPI_GETWORKAREA, 0, ctypes.byref(rect), 0)
    return rect


def probe(x, y, flags, items=12, wait=2.0):
    """在子线程里建窗口+菜单并弹出，主线程量出菜单窗口的真实 rect 后按 ESC 收掉。"""
    state = {}

    def worker():
        owner = u32.CreateWindowExW(
            0, "STATIC", "", WS_POPUP | WS_VISIBLE, 5, 5, 40, 40, None, None, None, None
        )
        menu = u32.CreatePopupMenu()
        for i in range(items):
            u32.AppendMenuW(menu, 0, 4000 + i, f"菜单项 {i + 1}")
        state["ret"] = u32.TrackPopupMenu(menu, flags, x, y, 0, owner, None)

    thread = threading.Thread(target=worker, daemon=True)
    thread.start()

    menu_hwnd = None
    deadline = time.time() + wait
    while time.time() < deadline and not menu_hwnd:
        menu_hwnd = u32.FindWindowW("#32768", None)
        time.sleep(0.02)
    if not menu_hwnd:
        return None
    time.sleep(0.2)  # 等它排好版
    rect = RECT()
    u32.GetWindowRect(menu_hwnd, ctypes.byref(rect))
    u32.PostMessageW(menu_hwnd, WM_KEYDOWN, VK_ESCAPE, 0)
    thread.join(timeout=2.0)
    return rect


def main():
    work = work_area()
    screen_h = u32.GetSystemMetrics(SM_CYSCREEN)
    taskbar = screen_h - work.bottom
    print(
        f"屏幕 {u32.GetSystemMetrics(SM_CXSCREEN)}x{screen_h}  "
        f"工作区 {work.left},{work.top}–{work.right},{work.bottom}  任务栏 {taskbar}px\n"
    )

    x_icon = work.right - 120
    y_icon = work.bottom + max(1, taskbar // 2)  # 托盘图标中心 ≈ 光标所在处

    cases = [
        ("tray-icon 原样 BOTTOMALIGN", x_icon, y_icon, TPM_BOTTOMALIGN | TPM_LEFTALIGN),
        ("muda 原样 LEFTALIGN", x_icon, y_icon, TPM_LEFTALIGN),
        ("AnyBoot 锚在工作区底边", x_icon, work.bottom, TPM_BOTTOMALIGN | TPM_LEFTALIGN),
    ]
    for tag, x, y, flags in cases:
        rect = probe(x, y, flags)
        if rect is None:
            print(f"[{tag}] 没抓到菜单窗口（TrackPopupMenu 可能失败了）")
            continue
        over = max(0, rect.bottom - work.bottom)
        verdict = f"★ 压住任务栏 {over}px" if over else "没压住"
        print(
            f"[{tag}] 锚点({x},{y}) → 菜单 {rect.left},{rect.top}–{rect.right},{rect.bottom}"
            f"  {verdict}"
        )
        time.sleep(0.3)


if __name__ == "__main__":
    main()
