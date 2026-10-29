//! 极简 ext4 解析：只读目录/文件 + **一个已存在文件的单块定点覆盖**。
//!
//! 为什么不用现成的 crate：我们要的东西窄到只有两件 ——
//! ① 按路径读出 `grub.cfg` 的文本；② 把 `grubenv` 的 1024 字节原样覆盖回
//! 它**原来那个数据块**。第 ② 件任何「只读」库都做不到，而「读写」库
//! （`rsext4` / `ext4_rs`）要么带整套日志与缓存、要么在稳定版编不过，
//! 为了这一件事引入它们的代价比自己解析 extent 更大。
//!
//! 边界（刻意收得很紧，越界就报错，绝不猜）：
//! * 只认 `ext4` / `ext3` / `ext2` 的常规布局；`extents` 与 `64bit` 必须支持，
//!   因为现代 `mke2fs` 默认都开；
//! * **不支持写元数据**：不分配块、不改 inode、不动位图/块组/超级块。
//!   唯一允许的写是「把一个已存在文件的前 N 字节覆盖进它自己的第 0 个数据块」；
//! * 文件系统挂着 `RECOVER`（上次没干净关机）时**只读允许、写入拒绝** ——
//!   没有日志回放的写会把未回放的旧元数据和新数据混在一起。
//!
//! 参考：Linux `fs/ext4/ext4.h` 的结构体定义（超级块、组描述符、inode、
//! extent 头/叶/索引、目录项）。

use std::fs::File;

// ─────────────────────────── 块设备 ───────────────────────────

/// 裸块来源。三种平台的差别只在「怎么打开」与「要不要对齐」，读取语义相同。
pub(crate) trait BlockSource {
    /// 按扇区对齐的单位（macOS 的 `/dev/rdisk*` 要求偏移与长度都是它的整数倍）
    fn sector(&self) -> u64 {
        512
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), String>;
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), String>;
    /// 能不能写（只读打开的设备 / 测试内存块）
    fn writable(&self) -> bool {
        false
    }
}

/// 内存块：单测用，也是「先把整块读进来再解析」的对照实现。
#[allow(dead_code)]
pub(crate) struct MemSource {
    pub(crate) data: Vec<u8>,
}

impl BlockSource for MemSource {
    fn sector(&self) -> u64 {
        1
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        let start = offset as usize;
        let end = start + buf.len();
        if end > self.data.len() {
            return Err(format!(
                "越界读：{start}..{end} 超过镜像大小 {}",
                self.data.len()
            ));
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), String> {
        let start = offset as usize;
        let end = start + buf.len();
        if end > self.data.len() {
            return Err(format!("越界写：{start}..{end}"));
        }
        self.data[start..end].copy_from_slice(buf);
        Ok(())
    }

    fn writable(&self) -> bool {
        true
    }
}

/// 真设备 / 镜像文件。
///
/// `align` 是「读写的偏移与长度必须是它的整数倍」的约束，只有 macOS 的
/// `/dev/rdisk*` 需要（实测：不对齐直接 `EINVAL`）；普通镜像文件与 Windows 的
/// `\\.\X:` 用 1 / 512 都能跑。
pub(crate) struct FileSource {
    file: File,
    /// 分区在整盘里的起始偏移（Windows 用 `\\.\PhysicalDriveN` 时才非 0）
    base: u64,
    align: u64,
    writable: bool,
}

impl FileSource {
    pub(crate) fn new(file: File, base: u64, align: u64) -> Self {
        Self {
            file,
            base,
            align: align.max(1),
            writable: true,
        }
    }

    /// macOS：`/dev/rdisk0s9` 这类裸分区节点，**只读**打开。
    ///
    /// 必须是只读：已挂载的卷用读写模式打开会直接 `EBUSY`（实测每块分区都
    /// 「打不开」，查了半天才发现是 open 的 flag 问题，不是权限问题）。
    #[cfg(target_os = "macos")]
    pub(crate) fn open_raw(path: &str) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|error| format!("打不开 {path}：{error}"))?;
        Ok(Self {
            writable: false,
            ..Self::new(file, 0, 512)
        })
    }

    /// Linux：裸分区节点（`/dev/nvme0n1p2`）**只读**打开。
    ///
    /// 与 macOS 不同：Linux 对已挂载分区的只读打开不 EBUSY，align 也不挑
    /// （那是 `/dev/rdisk*` 的怪癖）。这条只在免密助手子进程里跑到 ——
    /// 开 `/dev/*` 本来就要 root。
    #[cfg(target_os = "linux")]
    pub(crate) fn open_raw(path: &str) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|error| format!("打不开 {path}：{error}"))?;
        Ok(Self {
            writable: false,
            ..Self::new(file, 0, 1)
        })
    }

    /// macOS：读写方式打开裸分区（写 grubenv 用）。分区被挂载时会 EBUSY，
    /// 那种情况只能先卸载 —— 报错要说清楚，别让用户以为是权限问题。
    #[cfg(target_os = "macos")]
    pub(crate) fn open_raw_rw(path: &str) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| {
                format!(
                    "以读写方式打不开 {path}：{error}（这块分区若已被挂载会 EBUSY，\
                     需要先在磁盘工具里卸载它）"
                )
            })?;
        Ok(Self::new(file, 0, 512))
    }

    /// Linux：读写方式打开裸分区（写 grubenv 用）。只在 root 提权子进程里跑到
    /// （开 `/dev/*` 写本来就要 root）。与 macOS 不同：Linux 对已挂载分区的
    /// O_RDWR 打开不 EBUSY、align 也不挑 —— 但调用方必须自己先查挂载：
    /// 对已挂载分区裸写会绕过内核页缓存，能走挂载点文件就绝不裸写
    /// （见 `linux_grub::write_env_block` 的三档排序）。
    #[cfg(target_os = "linux")]
    pub(crate) fn open_raw_rw(path: &str) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| {
                let hint = if error.raw_os_error() == Some(13) {
                    "（裸设备写入要 root —— 这条只该在提权子进程里跑到）"
                } else {
                    ""
                };
                format!("以读写方式打不开 {path}：{error}{hint}")
            })?;
        Ok(Self::new(file, 0, 1))
    }

    /// 普通镜像文件（单测 / 排障用）
    #[allow(dead_code)]
    pub(crate) fn open_file(path: &str) -> Result<Self, String> {
        let file = File::open(path).map_err(|error| format!("打不开 {path}：{error}"))?;
        Ok(Self::new(file, 0, 1))
    }

    /// Windows：按分区偏移打开整盘 `\\.\PhysicalDriveN`。
    ///
    /// 为什么不沿用 `\\.\X:`：Windows 读不了 ext4，所以根本不会给 Linux 分区分配
    /// 盘符 —— 没有「分区级」路径可开，只能开整盘、用 `base` 把窗口推到分区起点。
    /// APFS 容器那条路（`apfs_reader` 的 `container_location`）用的是同一招。
    ///
    /// **只对提权进程有效**：`CreateFile` 打开物理磁盘非管理员一律
    /// `ERROR_ACCESS_DENIED (5)`。探测链路里这个函数只在提权子进程里被调用
    /// （见 `linux_grub::probe`），所以这里把错误原样带出来就够，别猜。
    #[cfg(target_os = "windows")]
    pub(crate) fn open_physical(disk: u32, base: u64, writable: bool) -> Result<Self, String> {
        let path = format!(r"\\.\PhysicalDrive{disk}");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(writable)
            .open(&path)
            .map_err(|error| {
                let hint = if error.raw_os_error() == Some(5) {
                    "（物理磁盘要管理员权限才能打开）"
                } else {
                    ""
                };
                format!("打不开整盘 {path}：{error}{hint}")
            })?;
        Ok(Self {
            writable,
            ..Self::new(file, base, 512)
        })
    }
}

impl BlockSource for FileSource {
    fn sector(&self) -> u64 {
        self.align
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        if self.align <= 1 {
            return pread_exact(&self.file, buf, self.base + offset);
        }
        // 对齐读：把区间扩到整个扇区，读进临时缓冲再切出来
        let align = self.align;
        let start = offset - offset % align;
        let end = {
            let raw = offset + buf.len() as u64;
            raw.div_ceil(align) * align
        };
        let mut scratch = vec![0u8; (end - start) as usize];
        pread_exact(&self.file, &mut scratch, self.base + start)?;
        let from = (offset - start) as usize;
        buf.copy_from_slice(&scratch[from..from + buf.len()]);
        Ok(())
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), String> {
        let align = self.align;
        if align <= 1 {
            return pwrite_all(&self.file, buf, self.base + offset);
        }
        // 对齐写：先读整扇区，改掉要改的那一段，再整扇区写回 ——
        // 裸设备的写同样是「整扇区」语义，只写半截会 EINVAL
        let start = offset - offset % align;
        let end = {
            let raw = offset + buf.len() as u64;
            raw.div_ceil(align) * align
        };
        let mut scratch = vec![0u8; (end - start) as usize];
        pread_exact(&self.file, &mut scratch, self.base + start)?;
        let from = (offset - start) as usize;
        scratch[from..from + buf.len()].copy_from_slice(buf);
        pwrite_all(&self.file, &scratch, self.base + start)
    }

    fn writable(&self) -> bool {
        self.writable
    }
}

#[cfg(unix)]
fn pread_exact(file: &File, buf: &mut [u8], offset: u64) -> Result<(), String> {
    use std::os::unix::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        let read = file
            .read_at(&mut buf[done..], offset + done as u64)
            .map_err(|error| format!("读设备失败（offset={}）：{error}", offset + done as u64))?;
        if read == 0 {
            return Err(format!(
                "读设备提前结束（offset={}，要到 {} 字节）",
                offset + done as u64,
                buf.len()
            ));
        }
        done += read;
    }
    Ok(())
}

#[cfg(unix)]
fn pwrite_all(file: &File, buf: &[u8], offset: u64) -> Result<(), String> {
    use std::os::unix::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        let wrote = file
            .write_at(&buf[done..], offset + done as u64)
            .map_err(|error| format!("写设备失败（offset={}）：{error}", offset + done as u64))?;
        if wrote == 0 {
            return Err("写设备没有进展".to_string());
        }
        done += wrote;
    }
    Ok(())
}

#[cfg(windows)]
fn pread_exact(file: &File, buf: &mut [u8], offset: u64) -> Result<(), String> {
    use std::os::windows::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        let read = file
            .seek_read(&mut buf[done..], offset + done as u64)
            .map_err(|error| format!("读设备失败（offset={}）：{error}", offset + done as u64))?;
        if read == 0 {
            return Err("读设备提前结束".to_string());
        }
        done += read;
    }
    Ok(())
}

#[cfg(windows)]
fn pwrite_all(file: &File, buf: &[u8], offset: u64) -> Result<(), String> {
    use std::os::windows::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        let wrote = file
            .seek_write(&buf[done..], offset + done as u64)
            .map_err(|error| format!("写设备失败（offset={}）：{error}", offset + done as u64))?;
        if wrote == 0 {
            return Err("写设备没有进展".to_string());
        }
        done += wrote;
    }
    Ok(())
}

// ─────────────────────────── 超级块 ───────────────────────────

pub(crate) const SUPERBLOCK_OFFSET: u64 = 1024;
pub(crate) const SUPERBLOCK_SIZE: usize = 1024;
/// `s_magic` 的值（偏移 0x38，小端 u16）
pub(crate) const MAGIC: u16 = 0xEF53;

/// `s_feature_incompat` 里我们认得且必须处理的位
const INCOMPAT_FILETYPE: u32 = 0x2;
const INCOMPAT_RECOVER: u32 = 0x4;
const INCOMPAT_EXTENTS: u32 = 0x40;
const INCOMPAT_64BIT: u32 = 0x80;
const INCOMPAT_ENCRYPT: u32 = 0x1_0000;

#[derive(Clone, Debug)]
pub(crate) struct Superblock {
    pub(crate) block_size: u32,
    pub(crate) blocks_count: u64,
    pub(crate) inodes_count: u32,
    pub(crate) first_data_block: u32,
    pub(crate) blocks_per_group: u32,
    pub(crate) inodes_per_group: u32,
    pub(crate) inode_size: u32,
    pub(crate) desc_size: u32,
    pub(crate) feature_incompat: u32,
    pub(crate) feature_ro_compat: u32,
    /// `84d0b19d-4ee2-4cd4-bc2c-80a77c19c702`（小写、带 `-`，与 `blkid` 一致）
    pub(crate) uuid: String,
    pub(crate) label: String,
    /// `s_last_mounted`：**只在 rw 挂载时更新**，不能当「上次挂在哪」的证据
    pub(crate) last_mounted: String,
}

impl Superblock {
    pub(crate) fn group_count(&self) -> u32 {
        if self.blocks_per_group == 0 {
            return 0;
        }
        let groups = (self.blocks_count + self.blocks_per_group as u64 - 1) / self.blocks_per_group as u64;
        groups.min(u32::MAX as u64) as u32
    }

    /// 上次没干净关机（journal 有待回放）：读可以，**写不行**
    pub(crate) fn needs_recovery(&self) -> bool {
        self.feature_incompat & INCOMPAT_RECOVER != 0
    }

    pub(crate) fn has_filetype(&self) -> bool {
        self.feature_incompat & INCOMPAT_FILETYPE != 0
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn cstr_at(bytes: &[u8], offset: usize, len: usize) -> String {
    let slice = &bytes[offset..offset + len];
    let end = slice.iter().position(|byte| *byte == 0).unwrap_or(slice.len());
    String::from_utf8_lossy(&slice[..end]).trim().to_string()
}

/// 解析超级块。只认 `s_magic == 0xEF53`，其余一律报错（不要「猜」）。
pub(crate) fn parse_superblock(bytes: &[u8]) -> Result<Superblock, String> {
    if bytes.len() < SUPERBLOCK_SIZE {
        return Err("超级块不足 1024 字节".to_string());
    }
    let magic = u16_at(bytes, 0x38);
    if magic != MAGIC {
        return Err(format!("不是 ext2/3/4（magic=0x{magic:04X}）"));
    }
    let log_block_size = u32_at(bytes, 0x18);
    if log_block_size > 6 {
        return Err(format!("块大小非法（2^{log_block_size}）"));
    }
    let block_size = 1024u32 << log_block_size;
    let incompat = u32_at(bytes, 0x60);
    let inode_size = if u32_at(bytes, 0x4C) == 0 {
        128
    } else {
        let size = u16_at(bytes, 0x58) as u32;
        if size == 0 { 128 } else { size }
    };
    // 64bit 打开之后 `s_desc_size` 才有意义（0 表示还是 32）
    let desc_size = if incompat & INCOMPAT_64BIT != 0 {
        let size = u16_at(bytes, 0xFE) as u32;
        if size == 0 { 64 } else { size }
    } else {
        32
    };
    let blocks_lo = u32_at(bytes, 0x04) as u64;
    let blocks_hi = if incompat & INCOMPAT_64BIT != 0 {
        u32_at(bytes, 0x150) as u64
    } else {
        0
    };
    let uuid_raw = &bytes[0x68..0x68 + 16];
    Ok(Superblock {
        block_size,
        blocks_count: blocks_lo | (blocks_hi << 32),
        inodes_count: u32_at(bytes, 0x00),
        first_data_block: u32_at(bytes, 0x14),
        blocks_per_group: u32_at(bytes, 0x20),
        inodes_per_group: u32_at(bytes, 0x28),
        inode_size,
        desc_size,
        feature_incompat: incompat,
        feature_ro_compat: u32_at(bytes, 0x64),
        uuid: format_uuid(uuid_raw),
        label: cstr_at(bytes, 0x78, 16),
        last_mounted: cstr_at(bytes, 0x88, 64),
    })
}

fn format_uuid(raw: &[u8]) -> String {
    let hex: String = raw.iter().map(|byte| format!("{byte:02x}")).collect();
    if hex.len() != 32 {
        return hex;
    }
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

// ─────────────────────────── inode ───────────────────────────

const INODE_MODE_DIR: u16 = 0x4000;
const INODE_MODE_REG: u16 = 0x8000;
const INODE_MODE_LINK: u16 = 0xA000;
/// `i_flags`：inode 用 extent 树而不是 12 个直接块 + 间接块
const INODE_FLAG_EXTENTS: u32 = 0x0008_0000;
const EXTENT_MAGIC: u16 = 0xF30A;

#[derive(Clone, Debug)]
pub(crate) struct Inode {
    pub(crate) number: u32,
    pub(crate) mode: u16,
    pub(crate) size: u64,
    pub(crate) flags: u32,
    /// `i_blocks`（512 字节为单位，只用于判断快符号链接）
    pub(crate) blocks_512: u32,
    /// `i_mtime`（epoch 秒）。自动模式没人同步文件，界面要说出这份 cfg 有多旧。
    pub(crate) mtime: i64,
    pub(crate) block: [u8; 60],
}

impl Inode {
    pub(crate) fn is_dir(&self) -> bool {
        self.mode & 0xF000 == INODE_MODE_DIR
    }
    pub(crate) fn is_file(&self) -> bool {
        self.mode & 0xF000 == INODE_MODE_REG
    }
    pub(crate) fn is_symlink(&self) -> bool {
        self.mode & 0xF000 == INODE_MODE_LINK
    }
    fn uses_extents(&self) -> bool {
        self.flags & INODE_FLAG_EXTENTS != 0
    }
}

fn parse_inode(number: u32, bytes: &[u8]) -> Inode {
    let mut block = [0u8; 60];
    block.copy_from_slice(&bytes[0x28..0x28 + 60]);
    let size_lo = u32_at(bytes, 0x04) as u64;
    let size_hi = u32_at(bytes, 0x6C) as u64;
    Inode {
        number,
        mode: u16_at(bytes, 0x00),
        // 只有普通文件才用 i_size_high；目录/链接的高位是别的含义，
        // 但 ext4 里目录尺寸不会超过 4G，直接用低位也安全
        size: size_lo | (size_hi << 32),
        flags: u32_at(bytes, 0x20),
        blocks_512: u32_at(bytes, 0x1C),
        // i_mtime 在 0x10（32 位）。2038 之后的高位在 extra 区，这里不读 ——
        // 显示用，差 30 年也比编一个时间好
        mtime: u32_at(bytes, 0x10) as i64,
        block,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DirEntry {
    pub(crate) name: String,
    pub(crate) inode: u32,
    /// 0 未知 / 1 普通文件 / 2 目录 / 7 符号链接
    pub(crate) file_type: u8,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FileMeta {
    pub(crate) size: u64,
    pub(crate) is_dir: bool,
    /// `i_mtime`（epoch 秒）
    pub(crate) mtime: i64,
}

// ─────────────────────────── 文件系统 ───────────────────────────

pub(crate) struct Ext4 {
    src: Box<dyn BlockSource>,
    pub(crate) sb: Superblock,
    /// 组描述符缓存（按需读，不整表预读：有的盘 100+ 组）
    groups: std::collections::HashMap<u32, (u64, u64)>, // group → (inode_table, block_bitmap)
}

impl Ext4 {
    pub(crate) fn open(mut src: Box<dyn BlockSource>) -> Result<Self, String> {
        let mut raw = vec![0u8; SUPERBLOCK_SIZE];
        src.read_at(SUPERBLOCK_OFFSET, &mut raw)?;
        let sb = parse_superblock(&raw)?;
        if sb.feature_incompat & INCOMPAT_ENCRYPT != 0 {
            return Err("这个 ext4 开了 per-file 加密（encrypt），读不了".to_string());
        }
        let mut fs = Self {
            src,
            sb,
            groups: std::collections::HashMap::new(),
        };
        // 「能开」必须是真的：组描述符与根 inode 都得读得出来，
        // 否则后面每个调用点都要各自处理这种半坏状态
        fs.group_desc(0)?;
        let root = fs.inode(2)?;
        if !root.is_dir() {
            return Err("根 inode 不是目录（镜像结构不对）".to_string());
        }
        Ok(fs)
    }

    pub(crate) fn block_size(&self) -> u32 {
        self.sb.block_size
    }

    pub(crate) fn is_writable(&self) -> bool {
        self.src.writable() && !self.sb.needs_recovery()
    }

    fn read_block(&mut self, block: u64) -> Result<Vec<u8>, String> {
        let size = self.sb.block_size as usize;
        if block >= self.sb.blocks_count {
            return Err(format!("块号 {block} 超出文件系统范围"));
        }
        let mut buf = vec![0u8; size];
        self.src.read_at(block * self.sb.block_size as u64, &mut buf)?;
        Ok(buf)
    }

    /// 组描述符在整个表里的字节偏移
    fn desc_offset(&self, group: u32) -> u64 {
        let table_block = self.sb.first_data_block as u64 + 1;
        table_block * self.sb.block_size as u64
            + group as u64 * self.sb.desc_size as u64
    }

    fn group_desc(&mut self, group: u32) -> Result<(u64, u64), String> {
        if let Some(hit) = self.groups.get(&group) {
            return Ok(*hit);
        }
        let offset = self.desc_offset(group);
        let mut buf = vec![0u8; self.sb.desc_size as usize];
        self.src.read_at(offset, &mut buf)?;
        let table_lo = u32_at(&buf, 0x08) as u64;
        let bitmap_lo = u32_at(&buf, 0x00) as u64;
        let (table_hi, bitmap_hi) = if self.sb.desc_size >= 64 {
            (u32_at(&buf, 0x28) as u64, u32_at(&buf, 0x20) as u64)
        } else {
            (0, 0)
        };
        let value = (table_lo | (table_hi << 32), bitmap_lo | (bitmap_hi << 32));
        self.groups.insert(group, value);
        Ok(value)
    }

    fn inode(&mut self, number: u32) -> Result<Inode, String> {
        if number == 0 || number > self.sb.inodes_count {
            return Err(format!("inode 号非法：{number}"));
        }
        let index = number - 1;
        let group = index / self.sb.inodes_per_group;
        let within = index % self.sb.inodes_per_group;
        let (table, _) = self.group_desc(group)?;
        let offset = table * self.sb.block_size as u64
            + within as u64 * self.sb.inode_size as u64;
        let mut buf = vec![0u8; self.sb.inode_size as usize];
        self.src.read_at(offset, &mut buf)?;
        Ok(parse_inode(number, &buf))
    }

    /// 逻辑块号 → 物理块号。`None` 表示这个逻辑块还没分配（稀疏文件）。
    fn map_block(&mut self, inode: &Inode, logical: u32) -> Result<Option<u64>, String> {
        if inode.uses_extents() {
            return self.map_extent(inode, logical);
        }
        // 老的间接块映射（ext2 时代）：12 个直接块 + 一级间接
        if logical < 12 {
            let block = u32_at(&inode.block, logical as usize * 4) as u64;
            return Ok(if block == 0 { None } else { Some(block) });
        }
        let per_block = (self.sb.block_size / 4) as u32;
        if logical < 12 + per_block {
            let indirect = u32_at(&inode.block, 12 * 4) as u64;
            if indirect == 0 {
                return Ok(None);
            }
            let table = self.read_block(indirect)?;
            let index = (logical - 12) as usize * 4;
            let block = u32_at(&table, index) as u64;
            return Ok(if block == 0 { None } else { Some(block) });
        }
        // 二级/三级间接极少见（>4G 的文件），明确报错而不是返回垃圾
        Err("文件用了二级/三级间接块映射，超出本模块能力".to_string())
    }

    fn map_extent(&mut self, inode: &Inode, logical: u32) -> Result<Option<u64>, String> {
        // 根节点在 i_block（60 字节），更高层节点是整块
        let root = inode.block.to_vec();
        map_extent_node(&root, logical, &mut |block| self.read_block(block), 5)
    }

    fn read_dir(&mut self, inode: &Inode) -> Result<Vec<DirEntry>, String> {
        if !inode.is_dir() {
            return Err("不是目录".to_string());
        }
        let block_size = self.sb.block_size;
        let blocks = inode.size.div_ceil(block_size as u64);
        let mut out = Vec::new();
        for logical in 0..blocks as u32 {
            let Some(physical) = self.map_block(inode, logical)? else {
                continue;
            };
            let data = self.read_block(physical)?;
            parse_dir_block(&data, self.sb.has_filetype(), &mut out);
        }
        Ok(out)
    }

    /// 列目录里的名字（去掉 `.` 与 `..`）
    pub(crate) fn list_dir(&mut self, path: &str) -> Result<Vec<String>, String> {
        let inode = self.lookup(path)?;
        let mut names: Vec<String> = self
            .read_dir(&inode)?
            .into_iter()
            .map(|entry| entry.name)
            .filter(|name| name != "." && name != "..")
            .collect();
        names.sort();
        Ok(names)
    }

    /// 按路径找 inode。路径分隔符恒为 `/`（ext4 内部就是这样）。
    pub(crate) fn lookup(&mut self, path: &str) -> Result<Inode, String> {
        let mut current = self.inode(2)?; // 根目录恒为 inode 2
        let mut pending: Vec<String> = path
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .map(str::to_string)
            .collect();
        let mut hops = 0;
        while let Some(name) = pending.first().cloned() {
            if name == ".." {
                pending.remove(0);
                // 简化的 `..` 处理：回根（我们只在 /boot/grub 这种浅路径上用）
                current = self.inode(2)?;
                continue;
            }
            if !current.is_dir() {
                return Err(format!("{name} 的父级不是目录"));
            }
            let hit = self
                .read_dir(&current)?
                .into_iter()
                .find(|entry| entry.name == name)
                .ok_or_else(|| format!("找不到 {name}"))?;
            let next = self.inode(hit.inode)?;
            pending.remove(0);
            if next.is_symlink() {
                hops += 1;
                if hops > 8 {
                    return Err("软链接层数过多".to_string());
                }
                let target = self.read_symlink(&next)?;
                let mut parts: Vec<String> = if target.starts_with('/') {
                    current = self.inode(2)?;
                    target
                        .split('/')
                        .filter(|part| !part.is_empty())
                        .map(str::to_string)
                        .collect()
                } else {
                    target
                        .split('/')
                        .filter(|part| !part.is_empty())
                        .map(str::to_string)
                        .collect()
                };
                parts.extend(pending);
                pending = parts;
                continue;
            }
            current = next;
        }
        Ok(current)
    }

    fn read_symlink(&mut self, inode: &Inode) -> Result<String, String> {
        // 快符号链接：目标直接存在 i_block 里（长度 ≤ 60 且没占数据块）
        if inode.size <= 60 && inode.blocks_512 == 0 {
            let len = inode.size as usize;
            return Ok(String::from_utf8_lossy(&inode.block[..len]).to_string());
        }
        let bytes = self.read_file(inode, 4096)?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    /// 读文件内容（上限 `cap` 字节；文件更大时截断并报错由调用方决定）
    pub(crate) fn read_file(&mut self, inode: &Inode, cap: usize) -> Result<Vec<u8>, String> {
        if inode.is_dir() {
            return Err("这是目录".to_string());
        }
        let size = inode.size.min(cap as u64) as usize;
        let block_size = self.sb.block_size as usize;
        let mut out = Vec::with_capacity(size);
        let mut logical = 0u32;
        while out.len() < size {
            let take = (size - out.len()).min(block_size);
            match self.map_block(inode, logical)? {
                Some(physical) => {
                    let data = self.read_block(physical)?;
                    out.extend_from_slice(&data[..take]);
                }
                // 稀疏洞：补零
                None => out.resize(out.len() + take, 0),
            }
            logical += 1;
        }
        Ok(out)
    }

    /// 读一个文件（按路径）。不存在返回 `None`，其余错误照常上抛。
    pub(crate) fn read_path(&mut self, path: &str, cap: usize) -> Result<Option<Vec<u8>>, String> {
        match self.lookup(path) {
            Ok(inode) if inode.is_file() => Ok(Some(self.read_file(&inode, cap)?)),
            Ok(_) => Err(format!("{path} 不是普通文件")),
            Err(error) if error.starts_with("找不到") => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn stat(&mut self, path: &str) -> Option<FileMeta> {
        match self.lookup(path) {
            Ok(inode) => Some(FileMeta {
                size: inode.size,
                is_dir: inode.is_dir(),
                mtime: inode.mtime,
            }),
            Err(_) => None,
        }
    }

    pub(crate) fn exists(&mut self, path: &str) -> bool {
        self.lookup(path).is_ok()
    }

    /// **定点覆盖**：把一个已存在文件的前 `bytes.len()` 字节换掉，只碰它的
    /// 第 0 个数据块，不动 inode 尺寸、不动位图、不动块组、不动超级块。
    ///
    /// 三个前提必须由调用方保证，这里也各校验一遍：
    /// ① 文件存在且是普通文件；② `i_size` 恰好等于 `bytes.len()`（定长文件，
    /// 长度一变就必须改 inode，那是另一条路）；③ 长度不超过一个块。
    pub(crate) fn overwrite_in_place(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        if !self.src.writable() {
            return Err("这个块来源是只读打开的".to_string());
        }
        if self.sb.needs_recovery() {
            return Err(
                "文件系统上次没干净关机（RECOVER 标志还在）：需要先在 Linux 里跑一次 \
                 fsck，或正常启动一次让它回放日志 —— 这期间写盘会把未回放的旧元数据和新数据混在一起"
                    .to_string(),
            );
        }
        let inode = self.lookup(path)?;
        if !inode.is_file() {
            return Err(format!("{path} 不是普通文件"));
        }
        if inode.size != bytes.len() as u64 {
            return Err(format!(
                "{path} 是 {} 字节，这次要写 {} 字节 —— 长度必须一致（定长覆盖）",
                inode.size,
                bytes.len()
            ));
        }
        if bytes.len() > self.sb.block_size as usize {
            return Err("要写的内容超过一个块".to_string());
        }
        let Some(physical) = self.map_block(&inode, 0)? else {
            return Err(format!("{path} 的第 0 个数据块没有分配"));
        };
        let block_size = self.sb.block_size as usize;
        let offset = physical * self.sb.block_size as u64;
        // 先整块读出：块尾可能还有这个文件的其余字节（grubenv 1024 < 4096）
        let mut block = vec![0u8; block_size];
        self.src.read_at(offset, &mut block)?;
        block[..bytes.len()].copy_from_slice(bytes);
        self.src.write_at(offset, &block)?;
        // 写完立刻回读逐字节比对 —— 写裸设备出错时系统常常「成功返回」
        let mut verify = vec![0u8; block_size];
        self.src.read_at(offset, &mut verify)?;
        if verify[..bytes.len()] != *bytes {
            return Err("写完回读不一致（设备可能没真正落盘）".to_string());
        }
        Ok(())
    }
}

/// 解析一个目录块里的所有有效项。
///
/// `htree` 的两个特例都能自然跳过：`dx_root` 的 `.`/`..` 是 rec_len 正确的假项，
/// 跳完就正好到块尾；`dx_node` 的假项 `inode=0`、`rec_len=块大小`，同样一跳到底。
fn parse_dir_block(data: &[u8], has_filetype: bool, out: &mut Vec<DirEntry>) {
    let mut offset = 0usize;
    while offset + 8 <= data.len() {
        let inode = u32_at(data, offset);
        let rec_len = u16_at(data, offset + 4) as usize;
        let (name_len, file_type) = if has_filetype {
            (data[offset + 6] as usize, data[offset + 7])
        } else {
            (u16_at(data, offset + 6) as usize, 0)
        };
        if rec_len < 8 || rec_len % 4 != 0 || offset + rec_len > data.len() {
            break;
        }
        if inode != 0 && name_len > 0 && offset + 8 + name_len <= data.len() {
            let name = String::from_utf8_lossy(&data[offset + 8..offset + 8 + name_len]).to_string();
            out.push(DirEntry {
                name,
                inode,
                file_type,
            });
        }
        offset += rec_len;
    }
}

/// extent 树的一层。`node` 是这一层的原始字节（根节点来自 `i_block` 的 60 字节，
/// 更高层是整块）；`read_block` 只在需要往下走一层时才被调用。
///
/// 用递归而不是循环：循环里「把读到的块借给下一轮」在 Rust 里要么写成引用自持
/// 结构体，要么每层都 clone 一份整块 —— 递归一份都不多拷，深度上限 5 也封死了。
fn map_extent_node(
    node: &[u8],
    logical: u32,
    read_block: &mut dyn FnMut(u64) -> Result<Vec<u8>, String>,
    depth_left: u32,
) -> Result<Option<u64>, String> {
    if node.len() < 12 || u16_at(node, 0) != EXTENT_MAGIC {
        return Err("extent 头 magic 不对".to_string());
    }
    if depth_left == 0 {
        return Err("extent 树比声明的更深".to_string());
    }
    let entries = u16_at(node, 2) as usize;
    let depth = u16_at(node, 6);
    if depth == 0 {
        for index in 0..entries {
            let at = 12 + index * 12;
            if at + 12 > node.len() {
                break;
            }
            let start = u32_at(node, at);
            let mut len = u16_at(node, at + 4) as u32;
            let high = u16_at(node, at + 6) as u64;
            let low = u32_at(node, at + 8) as u64;
            // 长度 > 32768 表示「未初始化的 extent」，内容应视为 0
            let uninit = len > 32768;
            if uninit {
                len -= 32768;
            }
            if len == 0 {
                continue;
            }
            if logical >= start && logical < start + len {
                if uninit {
                    return Ok(None);
                }
                // 一条 extent 覆盖连续多个逻辑块：物理块是「起点 + 块内偏移」，
                // 漏了这个偏移会让整段文件都读到第一个块（单测抓到的第一个 bug）
                let base = low | (high << 32);
                return Ok(Some(base + (logical - start) as u64));
            }
        }
        return Ok(None);
    }
    // 索引节点：取「start ≤ logical」里最后一条（等于二分，但条目少，直扫更省事）
    let mut chosen: Option<u64> = None;
    for index in 0..entries {
        let at = 12 + index * 12;
        if at + 12 > node.len() {
            break;
        }
        let start = u32_at(node, at);
        // 索引项实际只有 8 字节有效，但每条仍占 12 字节槽位
        let leaf_low = u32_at(node, at + 4) as u64;
        let leaf_high = u16_at(node, at + 8) as u64;
        if start <= logical {
            chosen = Some(leaf_low | (leaf_high << 32));
        } else {
            break;
        }
    }
    let Some(block) = chosen else {
        return Ok(None);
    };
    let child = read_block(block)?;
    map_extent_node(&child, logical, read_block, depth_left - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn superblock_bytes(block_size_log: u32, incompat: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; SUPERBLOCK_SIZE];
        bytes[0x00..0x04].copy_from_slice(&128u32.to_le_bytes()); // inodes_count
        bytes[0x04..0x08].copy_from_slice(&1_000_000u32.to_le_bytes()); // blocks_lo
        bytes[0x14..0x18].copy_from_slice(&0u32.to_le_bytes()); // first_data_block
        bytes[0x18..0x1C].copy_from_slice(&block_size_log.to_le_bytes());
        bytes[0x20..0x24].copy_from_slice(&32768u32.to_le_bytes()); // blocks_per_group
        bytes[0x28..0x2C].copy_from_slice(&8192u32.to_le_bytes()); // inodes_per_group
        bytes[0x38..0x3A].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[0x4C..0x50].copy_from_slice(&1u32.to_le_bytes()); // rev_level = DYNAMIC
        bytes[0x58..0x5A].copy_from_slice(&256u16.to_le_bytes()); // inode_size
        bytes[0x60..0x64].copy_from_slice(&incompat.to_le_bytes());
        bytes[0x68..0x78].copy_from_slice(&[
            0x84, 0xd0, 0xb1, 0x9d, 0x4e, 0xe2, 0x4c, 0xd4, 0xbc, 0x2c, 0x80, 0xa7, 0x7c, 0x19,
            0xc7, 0x02,
        ]);
        bytes[0x78..0x78 + 8].copy_from_slice(b"arch-gui");
        bytes[0x88..0x88 + 6].copy_from_slice(b"/sysro");
        if incompat & INCOMPAT_64BIT != 0 {
            bytes[0xFE..0x100].copy_from_slice(&64u16.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn superblock_fields_match_blkid() {
        let bytes = superblock_bytes(2, INCOMPAT_64BIT | INCOMPAT_EXTENTS | INCOMPAT_FILETYPE);
        let sb = parse_superblock(&bytes).unwrap();
        assert_eq!(sb.block_size, 4096);
        assert_eq!(sb.blocks_count, 1_000_000);
        assert_eq!(sb.inode_size, 256);
        assert_eq!(sb.desc_size, 64);
        assert_eq!(sb.uuid, "84d0b19d-4ee2-4cd4-bc2c-80a77c19c702");
        assert_eq!(sb.label, "arch-gui");
        assert_eq!(sb.last_mounted, "/sysro");
        assert!(sb.has_filetype());
        assert!(!sb.needs_recovery());
        // 100 万块 / 32768 每块 = 31 组（向上取整）
        assert_eq!(sb.group_count(), 31);
    }

    #[test]
    fn block_size_and_recovery_flag() {
        let sb = parse_superblock(&superblock_bytes(0, 0)).unwrap();
        assert_eq!(sb.block_size, 1024);
        assert_eq!(sb.desc_size, 32);
        let dirty = parse_superblock(&superblock_bytes(2, INCOMPAT_RECOVER)).unwrap();
        assert!(dirty.needs_recovery());
    }

    #[test]
    fn rejects_non_ext_filesystems() {
        let mut bytes = superblock_bytes(2, 0);
        bytes[0x38..0x3A].copy_from_slice(&0x5346u16.to_le_bytes()); // 假 magic
        let error = parse_superblock(&bytes).unwrap_err();
        assert!(error.contains("不是 ext2/3/4"), "{error}");
    }

    #[test]
    fn dir_block_walks_rec_len() {
        // 三项：`.`、`..`、`grub`
        let mut block = vec![0u8; 4096];
        let mut put = |offset: usize, inode: u32, rec_len: u16, name: &[u8], file_type: u8| {
            block[offset..offset + 4].copy_from_slice(&inode.to_le_bytes());
            block[offset + 4..offset + 6].copy_from_slice(&rec_len.to_le_bytes());
            block[offset + 6] = name.len() as u8;
            block[offset + 7] = file_type;
            block[offset + 8..offset + 8 + name.len()].copy_from_slice(name);
        };
        put(0, 2, 12, b".", 2);
        put(12, 2, 12, b"..", 2);
        put(24, 131, 4096 - 24, b"grub", 2);
        let mut out = Vec::new();
        parse_dir_block(&block, true, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].name, "grub");
        assert_eq!(out[2].inode, 131);
        assert_eq!(out[2].file_type, 2);
    }

    #[test]
    fn dir_block_skips_deleted_and_bad_rec_len() {
        let mut block = vec![0u8; 4096];
        // 第一项 inode=0（已删除），rec_len=16 → 跳过但继续
        block[0..4].copy_from_slice(&0u32.to_le_bytes());
        block[4..6].copy_from_slice(&16u16.to_le_bytes());
        // 第二项 rec_len=0 → 立刻停，避免死循环
        block[16..20].copy_from_slice(&9u32.to_le_bytes());
        block[20..22].copy_from_slice(&0u16.to_le_bytes());
        let mut out = Vec::new();
        parse_dir_block(&block, true, &mut out);
        assert!(out.is_empty());
    }

    /// 构造一个 extent 头 + `entries` 条 12 字节条目
    fn extent_node(entries: u16, depth: u16, body: &[u8]) -> Vec<u8> {
        let mut node = vec![0u8; 12];
        node[0..2].copy_from_slice(&EXTENT_MAGIC.to_le_bytes());
        node[2..4].copy_from_slice(&entries.to_le_bytes());
        node[4..6].copy_from_slice(&entries.to_le_bytes());
        node[6..8].copy_from_slice(&depth.to_le_bytes());
        node.extend_from_slice(body);
        node
    }

    fn leaf_entry(logical: u32, len: u16, physical: u64) -> Vec<u8> {
        let mut body = vec![0u8; 12];
        body[0..4].copy_from_slice(&logical.to_le_bytes());
        body[4..6].copy_from_slice(&len.to_le_bytes());
        body[6..8].copy_from_slice(&((physical >> 32) as u16).to_le_bytes());
        body[8..12].copy_from_slice(&(physical as u32).to_le_bytes());
        body
    }

    fn index_entry(logical: u32, child: u64) -> Vec<u8> {
        let mut body = vec![0u8; 12];
        body[0..4].copy_from_slice(&logical.to_le_bytes());
        body[4..8].copy_from_slice(&(child as u32).to_le_bytes());
        body[8..10].copy_from_slice(&((child >> 32) as u16).to_le_bytes());
        body
    }

    #[test]
    fn extent_leaf_locates_logical_block() {
        let body: Vec<u8> = [
            leaf_entry(0, 2, 0x1234),
            leaf_entry(2, 1, 0x2000),
        ]
        .concat();
        let node = extent_node(2, 0, &body);
        let mut never = |_block: u64| -> Result<Vec<u8>, String> {
            Err("叶子节点不该再往下读".to_string())
        };
        assert_eq!(map_extent_node(&node, 0, &mut never, 5).unwrap(), Some(0x1234));
        assert_eq!(map_extent_node(&node, 1, &mut never, 5).unwrap(), Some(0x1235));
        assert_eq!(map_extent_node(&node, 2, &mut never, 5).unwrap(), Some(0x2000));
        assert_eq!(map_extent_node(&node, 3, &mut never, 5).unwrap(), None);
    }

    #[test]
    fn extent_index_descends_one_level() {
        // 根：depth=1，两条索引 → 0x9000 / 0x9001；0x9000 里才是叶子
        let root_body: Vec<u8> = [
            index_entry(0, 0x9000),
            index_entry(100, 0x9001),
        ]
        .concat();
        let root = extent_node(2, 1, &root_body);
        let leaf_zero: Vec<u8> = [
            leaf_entry(0, 4, 0x5000),
        ]
        .concat();
        let leaf_block = extent_node(1, 0, &leaf_zero);

        let mut reads: Vec<u64> = Vec::new();
        let mut reader = |block: u64| -> Result<Vec<u8>, String> {
            reads.push(block);
            if block == 0x9000 {
                Ok(leaf_block.clone())
            } else {
                Err(format!("没造这块：{block}"))
            }
        };
        assert_eq!(
            map_extent_node(&root, 3, &mut reader, 5).unwrap(),
            Some(0x5003)
        );
        assert_eq!(reads, vec![0x9000]);
    }

    #[test]
    fn uninitialized_extent_reads_as_hole() {
        // len 带 32768 标志 = 未初始化，内容视为全零
        let node = extent_node(1, 0, &leaf_entry(0, 32768 + 4, 0x7000));
        let mut never = |_block: u64| -> Result<Vec<u8>, String> { Err("不该读".to_string()) };
        assert_eq!(map_extent_node(&node, 0, &mut never, 5).unwrap(), None);
    }

    #[test]
    fn bad_extent_magic_is_loud() {
        let node = vec![0u8; 60];
        let mut never = |_block: u64| -> Result<Vec<u8>, String> { Err("不该读".to_string()) };
        let error = map_extent_node(&node, 0, &mut never, 5).unwrap_err();
        assert!(error.contains("magic"), "{error}");
    }
}
