use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, anyhow, bail};
use rustix::fs::{AtFlags, CWD, Mode, OFlags, StatVfsMountFlags, StatxAttributes, StatxFlags};
use rustix::io::Errno;
use rustix::ioctl::{self, Getter, Setter, opcode};
use rustix::mount::{FsMountFlags, FsOpenFlags, MountAttrFlags, MountFlags, MountPropagationFlags, MoveMountFlags, UnmountFlags};

pub fn run(cmd: &mut Command) -> Result<Output> {
    let out = cmd.output().with_context(|| format!("run {cmd:?}"))?;
    if !out.status.success() {
        bail!("{cmd:?}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out)
}

pub fn open_dir(p: &Path) -> io::Result<OwnedFd> {
    Ok(rustix::fs::open(p, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())?)
}

const OVERLAY_OPTS: &[(&str, &str)] = &[("redirect_dir", "on"), ("metacopy", "off"), ("index", "off"), ("xino", "auto")];

fn fs_log(fs: &OwnedFd) -> String {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    for _ in 0..32 {
        match rustix::io::read(fs, &mut buf) {
            Ok(n) if n > 0 => out.push(String::from_utf8_lossy(&buf[..n]).into_owned()),
            _ => break,
        }
    }
    out.join("; ")
}

pub fn overlay(lowers: &[PathBuf], upper: Option<(PathBuf, PathBuf)>) -> Result<OwnedFd> {
    use rustix::mount::{fsconfig_create, fsconfig_set_flag, fsconfig_set_string};
    let fs = rustix::mount::fsopen("overlay", FsOpenFlags::FSOPEN_CLOEXEC).context("fsopen(overlay)")?;
    let wrap = |what: String, e: Errno, fs: &OwnedFd| anyhow!("{what}: {} [{}]", io::Error::from(e), fs_log(fs));
    for p in lowers {
        fsconfig_set_string(&fs, "lowerdir+", p).map_err(|e| wrap(format!("lowerdir+ {}", p.display()), e, &fs))?;
    }
    if let Some((upper, work)) = &upper {
        fsconfig_set_string(&fs, "upperdir", upper).map_err(|e| wrap("upperdir".into(), e, &fs))?;
        fsconfig_set_string(&fs, "workdir", work).map_err(|e| wrap("workdir".into(), e, &fs))?;
    }
    let ro = upper.is_none();
    for (k, v) in OVERLAY_OPTS {
        if ro && (*k == "index" || *k == "metacopy") {
            continue;
        }
        fsconfig_set_string(&fs, *k, *v).map_err(|e| wrap((*k).into(), e, &fs))?;
    }
    if ro {
        fsconfig_set_flag(&fs, "ro").ok();
    }
    fsconfig_create(&fs).map_err(|e| wrap("create overlay".into(), e, &fs))?;
    let attrs = if ro { MountAttrFlags::MOUNT_ATTR_RDONLY } else { MountAttrFlags::empty() };
    let m = rustix::mount::fsmount(&fs, FsMountFlags::FSMOUNT_CLOEXEC, attrs).map_err(|e| wrap("fsmount".into(), e, &fs))?;
    if !ro && fd_readonly(&m)? {
        bail!("overlay came up read-only (upper not writable?) [{}]", fs_log(&fs));
    }
    Ok(m)
}

fn fd_readonly(fd: &OwnedFd) -> io::Result<bool> {
    Ok(rustix::fs::fstatvfs(fd)?.f_flag.contains(StatVfsMountFlags::RDONLY))
}

pub fn move_mount(fd: &OwnedFd, target: &Path) -> io::Result<()> {
    Ok(rustix::mount::move_mount(fd, "", CWD, target, MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH)?)
}

pub fn umount(target: &Path, detach: bool) -> io::Result<()> {
    let flags = if detach { UnmountFlags::DETACH } else { UnmountFlags::empty() } | UnmountFlags::NOFOLLOW;
    Ok(rustix::mount::unmount(target, flags)?)
}

pub fn unmount(p: &Path) -> io::Result<()> {
    if is_mountpoint(p) {
        umount_retry(p)?;
    }
    Ok(())
}

pub fn unmount_and_remove(p: &Path) -> io::Result<()> {
    unmount(p)?;
    let _ = std::fs::remove_dir(p);
    Ok(())
}

pub fn exchange(a: &Path, b: &Path) -> io::Result<()> {
    Ok(rustix::fs::renameat_with(CWD, a, CWD, b, rustix::fs::RenameFlags::EXCHANGE)?)
}

pub fn users_of(dir: &Path) -> Vec<String> {
    let me = std::process::id().to_string();
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let pid = e.file_name().to_string_lossy().into_owned();
        if pid == me || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let p = e.path();
        let mut links = vec![p.join("cwd"), p.join("root")];
        links.extend(std::fs::read_dir(p.join("fd")).into_iter().flatten().flatten().map(|f| f.path()));
        if links.iter().any(|l| std::fs::read_link(l).is_ok_and(|t| t.starts_with(dir))) {
            let comm = std::fs::read_to_string(p.join("comm")).unwrap_or_default();
            out.push(format!("{pid} ({})", comm.trim()));
        }
    }
    out
}

fn umount_retry(target: &Path) -> io::Result<()> {
    for _ in 0..20 {
        match umount(target, false) {
            Ok(()) => return Ok(()),
            Err(e) if Errno::from_io_error(&e) == Some(Errno::BUSY) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(e),
        }
    }
    eprintln!("tracefs-daemon: {} busy, detaching", target.display());
    umount(target, true)
}

pub fn is_mountpoint(p: &Path) -> bool {
    rustix::fs::statx(CWD, p, AtFlags::SYMLINK_NOFOLLOW, StatxFlags::empty())
        .is_ok_and(|st| st.stx_attributes_mask.contains(StatxAttributes::MOUNT_ROOT) && st.stx_attributes.contains(StatxAttributes::MOUNT_ROOT))
}

pub fn mount_private_tmpfs(p: &Path) -> Result<()> {
    rustix::mount::mount("tmpfs", p, "tmpfs", MountFlags::NOSUID | MountFlags::NODEV, c"mode=0711,size=64m,nr_inodes=65536")
        .with_context(|| format!("mount tmpfs on {}", p.display()))?;
    rustix::mount::mount_change(p, MountPropagationFlags::PRIVATE).context("make run dir private")?;
    Ok(())
}

pub fn is_btrfs(p: &Path) -> io::Result<bool> {
    const BTRFS_SUPER_MAGIC: i64 = 0x9123_683e;
    Ok(rustix::fs::statfs(p)?.f_type as i64 == BTRFS_SUPER_MAGIC)
}

#[repr(C)]
struct VolArgsV2 {
    fd: i64,
    transid: u64,
    flags: u64,
    unused: [u64; 4],
    name: [u8; 4040],
}

#[repr(C)]
struct VolArgs {
    fd: i64,
    name: [u8; 4088],
}

const BTRFS_IOCTL_MAGIC: u8 = 0x94;
const BTRFS_SUBVOL_RDONLY: u64 = 1 << 1;
const BTRFS_IOC_SNAP_DESTROY: ioctl::Opcode = opcode::write::<VolArgs>(BTRFS_IOCTL_MAGIC, 15);
const BTRFS_IOC_SNAP_CREATE_V2: ioctl::Opcode = opcode::write::<VolArgsV2>(BTRFS_IOCTL_MAGIC, 23);
const BTRFS_IOC_SUBVOL_CREATE_V2: ioctl::Opcode = opcode::write::<VolArgsV2>(BTRFS_IOCTL_MAGIC, 24);
const BTRFS_IOC_SUBVOL_GETFLAGS: ioctl::Opcode = opcode::read::<u64>(BTRFS_IOCTL_MAGIC, 25);
const BTRFS_IOC_SUBVOL_SETFLAGS: ioctl::Opcode = opcode::write::<u64>(BTRFS_IOCTL_MAGIC, 26);

fn split(p: &Path) -> io::Result<(OwnedFd, &[u8])> {
    let parent = p.parent().ok_or(Errno::INVAL)?;
    let name = p.file_name().ok_or(Errno::INVAL)?.as_bytes();
    Ok((open_dir(parent)?, name))
}

fn v2(name: &[u8]) -> io::Result<VolArgsV2> {
    if name.is_empty() || name.len() >= 4040 || name.contains(&b'/') {
        return Err(Errno::INVAL.into());
    }
    let mut a = VolArgsV2 { fd: 0, transid: 0, flags: 0, unused: [0; 4], name: [0; 4040] };
    a.name[..name.len()].copy_from_slice(name);
    Ok(a)
}

pub fn subvol_create(p: &Path) -> io::Result<()> {
    let (dir, name) = split(p)?;
    let a = v2(name)?;
    Ok(unsafe { ioctl::ioctl(&dir, Setter::<BTRFS_IOC_SUBVOL_CREATE_V2, _>::new(a)) }?)
}

pub fn snapshot(src: &Path, dst: &Path, readonly: bool) -> io::Result<()> {
    let s = open_dir(src)?;
    let (dir, name) = split(dst)?;
    let mut a = v2(name)?;
    a.fd = s.as_raw_fd().into();
    a.flags = if readonly { BTRFS_SUBVOL_RDONLY } else { 0 };
    Ok(unsafe { ioctl::ioctl(&dir, Setter::<BTRFS_IOC_SNAP_CREATE_V2, _>::new(a)) }?)
}

pub fn subvol_delete(p: &Path) -> io::Result<()> {
    let (dir, name) = split(p)?;
    if name.len() >= 4088 {
        return Err(Errno::INVAL.into());
    }
    let mut a = VolArgs { fd: 0, name: [0; 4088] };
    a.name[..name.len()].copy_from_slice(name);
    Ok(unsafe { ioctl::ioctl(&dir, Setter::<BTRFS_IOC_SNAP_DESTROY, _>::new(a)) }?)
}

pub fn set_readonly(p: &Path) -> io::Result<()> {
    let fd = open_dir(p)?;
    let flags = unsafe { ioctl::ioctl(&fd, Getter::<BTRFS_IOC_SUBVOL_GETFLAGS, u64>::new()) }? | BTRFS_SUBVOL_RDONLY;
    Ok(unsafe { ioctl::ioctl(&fd, Setter::<BTRFS_IOC_SUBVOL_SETFLAGS, u64>::new(flags)) }?)
}
