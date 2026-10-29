//! 裸盘写入前的「锁卷 + 卸载卷」（Windows 专属）。
//!
//! Windows 的规矩（MSDN「Blocking Direct Write Operations to Volumes and Disks」）：
//! 对 `\\.\PhysicalDriveN` 的裸写，凡是落在**已挂载卷**的扇区上，
//! 一律 `ERROR_ACCESS_DENIED(5)` —— 读不拦、写拦。「已挂载」跟有没有盘符无关：
//! ESP 的 FAT 卷就算没分配字母，系统也是挂载着的；第三方 ext4 驱动装的卷同理。
//! 2026-10-30 真机实测：GRUB 探测（只读）全绿，一写环境块就 os error 5。
//!
//! 解法是官方那套流程：找到盖住目标偏移的卷 → `FSCTL_DISMOUNT_VOLUME`
//! （MSDN 说 lock **或** dismount 任一即可解除拦截）→ 尽力 `FSCTL_LOCK_VOLUME`
//! → 写 → RAII 放开。卷被卸载后，系统会在下一次访问时自动重新挂载，
//! 不需要（也不该）在这里做任何恢复动作。
//!
//! 为什么不用「先开卷写」绕过：规则对磁盘句柄和卷句柄一视同仁，
//! 唯一的官方豁免就是 lock/dismount。

#![cfg(target_os = "windows")]

use std::mem::size_of;
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose,
    IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, FSCTL_UNLOCK_VOLUME, DISK_EXTENT,
    VOLUME_DISK_EXTENTS,
};

/// 最多比对的盘区数（一条卷横跨 8 块盘还没见着过，够用）。
const MAX_EXTENTS: usize = 8;

/// 一条已卸载（并尽力锁定）的卷。活着 = 卷保持卸载；drop = 放开。
pub(crate) struct MountedVolumeGuard {
    handle: windows_sys::Win32::Foundation::HANDLE,
    locked: bool,
}

impl Drop for MountedVolumeGuard {
    fn drop(&mut self) {
        unsafe {
            if self.locked {
                let mut returned = 0u32;
                let _ = DeviceIoControl(
                    self.handle,
                    FSCTL_UNLOCK_VOLUME,
                    std::ptr::null(),
                    0,
                    std::ptr::null_mut(),
                    0,
                    &mut returned,
                    std::ptr::null_mut(),
                );
            }
            let _ = CloseHandle(self.handle);
        }
    }
}

/// 找到「盘号 `disk` 上从 `offset` 开始的那块分区」所属的卷，把它卸载（并尽力锁定），
/// 让随后的 `\\.\PhysicalDriveN` 裸写不再吃 `ERROR_ACCESS_DENIED`。
///
/// 找不到覆盖该偏移的卷（卷对象不存在）→ 报错，宁可不写也不裸奔。
pub(crate) fn lock_extent(disk: u32, offset: u64) -> Result<MountedVolumeGuard, String> {
    // \\?\Volume{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}\ —— 远短于 MAX_PATH，
    // 按 MSDN 样例给 261 一劳永逸。
    let mut name = [0u16; 261];
    unsafe {
        let find = FindFirstVolumeW(name.as_mut_ptr(), name.len() as u32);
        if find == INVALID_HANDLE_VALUE {
            return Err(format!(
                "枚举卷失败（Win32 错误 {}）：没法在裸写前卸载目标卷",
                windows_sys::Win32::Foundation::GetLastError()
            ));
        }

        let mut more = 1i32; // windows-sys 的 BOOL
        let mut tried = 0usize;
        while more != 0 {
            let len = name.iter().position(|&c| c == 0).unwrap_or(0);
            let path = String::from_utf16_lossy(&name[..len]);
            // FindFirstVolumeW 给的路径带结尾反斜杠；CreateFileW 开卷要截掉，
            // 否则它当目录开（文档明说的坑）。
            let open_path = path.strip_suffix('\\').unwrap_or(&path);
            match try_lock(open_path, disk, offset) {
                Some(Ok(guard)) => {
                    FindVolumeClose(find);
                    return Ok(guard);
                }
                // 就是这个卷但卸载失败：带真实原因直接报，别再翻别的卷
                Some(Err(error)) => {
                    FindVolumeClose(find);
                    return Err(error);
                }
                None => {} // 不是这个卷，继续找
            }
            tried += 1;
            more = FindNextVolumeW(find, name.as_mut_ptr(), name.len() as u32);
        }
        FindVolumeClose(find);

        let looked_at = if tried == 1 { "1 个卷" } else { &format!("{tried} 个卷") };
        Err(format!(
            "盘 {disk} 偏移 {offset} 找不到所属卷（比对了 {looked_at}）：\
             裸写前必须卸载目标卷，否则 Windows 会以「拒绝访问」拦下写入"
        ))
    }
}

/// 尝试把 `path` 锁住。返回：
/// * `Ok(Some(guard))` —— 就是这个卷，已经卸载；
/// * `Ok(None)` —— 开不了 / 不是盖这个偏移的卷，调用方继续枚举；
/// * `Err(reason)` —— 就是这个卷但卸载失败，带真实原因给用户。
fn try_lock(path: &str, disk: u32, offset: u64) -> Option<Result<MountedVolumeGuard, String>> {
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    unsafe {
        let handle = CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            // 系统上有开不了（或不想开）的卷很正常，换下一个比对。
            return None;
        }

        // 这卷盖着哪些盘区？
        let mut buffer = [0u8; size_of::<VOLUME_DISK_EXTENTS>() + MAX_EXTENTS * size_of::<DISK_EXTENT>()];
        let mut returned = 0u32;
        let ok = DeviceIoControl(
            handle,
            IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
            std::ptr::null(),
            0,
            buffer.as_mut_ptr() as *mut std::ffi::c_void,
            buffer.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        );
        if ok == 0 {
            let _ = CloseHandle(handle);
            return None;
        }
        let extents = &*(buffer.as_ptr() as *const VOLUME_DISK_EXTENTS);
        let count = (extents.NumberOfDiskExtents as usize).min(MAX_EXTENTS);
        let hit = extents.Extents[..count].iter().any(|extent: &DISK_EXTENT| {
            extent.DiskNumber == disk
                && (offset as i64) >= extent.StartingOffset
                && (offset as i64) < extent.StartingOffset + extent.ExtentLength
        });
        if !hit {
            let _ = CloseHandle(handle);
            return None;
        }

        // 命中。dismount 是解除裸写拦截的必要动作；lock 是锦上添花
        //（系统卷上有别的句柄时 lock 会失败，dismount 成功就够用了）。
        let mut returned = 0u32;
        if DeviceIoControl(
            handle,
            FSCTL_DISMOUNT_VOLUME,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        ) == 0
        {
            let error = windows_sys::Win32::Foundation::GetLastError();
            let _ = CloseHandle(handle);
            return Some(Err(format!(
                "卷 {path} 卸载失败（Win32 错误 {error}）——有程序正占着这个卷？\
                 关掉占用的程序后重试"
            )));
        }
        let mut returned = 0u32;
        let locked = DeviceIoControl(
            handle,
            FSCTL_LOCK_VOLUME,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        ) != 0;

        Some(Ok(MountedVolumeGuard { handle, locked }))
    }
}
