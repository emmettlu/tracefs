use std::cell::RefCell;
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::path::Path;
use std::rc::Rc;

use acfs_core::merge::{FType, Meta};
use acfs_core::path;
use rustix::fs::{AtFlags, Dir, FileType, Gid, Mode, OFlags, RenameFlags, ResolveFlags, Timespec, Timestamps, Uid, XattrFlags};
use rustix::io::Errno;

const RESOLVE: ResolveFlags = ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS).union(ResolveFlags::NO_MAGICLINKS).union(ResolveFlags::NO_XDEV);

#[derive(Clone, Copy, Debug)]
pub struct Stat {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub ino: u64,
    pub rdev: u64,
    pub atime: (i64, i64),
    pub mtime: (i64, i64),
    pub ctime: (i64, i64),
}

impl Stat {
    pub fn ftype(&self) -> FType {
        match FileType::from_raw_mode(self.mode) {
            FileType::RegularFile => FType::Reg,
            FileType::Directory => FType::Dir,
            FileType::Symlink => FType::Symlink,
            FileType::Fifo => FType::Fifo,
            _ => FType::Other,
        }
    }
    pub fn is_dir(&self) -> bool {
        self.ftype() == FType::Dir
    }
    pub fn is_whiteout(&self) -> bool {
        FileType::from_raw_mode(self.mode) == FileType::CharacterDevice && self.rdev == 0
    }
    pub fn meta(&self) -> Meta {
        Meta { mode: self.mode & 0o7777, uid: self.uid, gid: self.gid }
    }
    fn from(st: &rustix::fs::Stat) -> Stat {
        Stat {
            mode: st.st_mode,
            uid: st.st_uid,
            gid: st.st_gid,
            size: st.st_size as u64,
            ino: st.st_ino,
            rdev: st.st_rdev,
            atime: (st.st_atime, st.st_atime_nsec as i64),
            mtime: (st.st_mtime, st.st_mtime_nsec as i64),
            ctime: (st.st_ctime, st.st_ctime_nsec as i64),
        }
    }
}

fn absent(e: &io::Error) -> bool {
    matches!(Errno::from_io_error(e), Some(Errno::NOENT | Errno::NOTDIR | Errno::LOOP | Errno::XDEV))
}

pub fn timestamps(atime: (i64, i64), mtime: (i64, i64)) -> Timestamps {
    Timestamps { last_access: Timespec { tv_sec: atime.0, tv_nsec: atime.1 }, last_modification: Timespec { tv_sec: mtime.0, tv_nsec: mtime.1 } }
}

type OpenDir = (Vec<u8>, Rc<OwnedFd>);

pub struct Tree {
    root: OwnedFd,
    last: Option<RefCell<Option<OpenDir>>>,
}

impl Tree {
    pub fn open(p: &Path) -> io::Result<Tree> {
        let fd = rustix::fs::open(p, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW, Mode::empty())?;
        Ok(Tree { root: fd, last: None })
    }

    pub fn open_ro(p: &Path) -> io::Result<Tree> {
        Ok(Tree { last: Some(RefCell::new(None)), ..Tree::open(p)? })
    }

    pub fn dir(&self, rel: &[u8]) -> io::Result<OwnedFd> {
        if rel.is_empty() {
            return Ok(rustix::fs::openat(&self.root, ".", OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())?);
        }
        Ok(rustix::fs::openat2(&self.root, rel, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty(), RESOLVE)?)
    }

    fn parent<'a>(&self, rel: &'a [u8]) -> io::Result<(Rc<OwnedFd>, &'a [u8])> {
        let (dir, name) = (path::parent(rel), path::file_name(rel));
        let Some(last) = &self.last else { return Ok((Rc::new(self.dir(dir)?), name)) };
        if let Some((p, fd)) = &*last.borrow()
            && p == dir
        {
            return Ok((fd.clone(), name));
        }
        let fd = Rc::new(self.dir(dir)?);
        *last.borrow_mut() = Some((dir.to_vec(), fd.clone()));
        Ok((fd, name))
    }

    pub fn lstat(&self, rel: &[u8]) -> io::Result<Option<Stat>> {
        if rel.is_empty() {
            return Ok(Some(Stat::from(&rustix::fs::fstat(&self.root)?)));
        }
        let (dir, name) = match self.parent(rel) {
            Ok(x) => x,
            Err(e) if absent(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        match rustix::fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from) {
            Ok(st) => Ok(Some(Stat::from(&st))),
            Err(e) if absent(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn list(&self, rel: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        let fd = match self.dir(rel) {
            Ok(fd) => fd,
            Err(e) if absent(&e) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for ent in Dir::new(fd)? {
            let name = ent?.file_name().to_bytes().to_vec();
            if name != b"." && name != b".." {
                out.push(name);
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn open_entry(&self, rel: &[u8], flags: OFlags) -> io::Result<OwnedFd> {
        if rel.is_empty() {
            return self.dir(rel);
        }
        let (dir, name) = self.parent(rel)?;
        Ok(rustix::fs::openat(&dir, name, flags | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NOCTTY, Mode::empty())?)
    }

    pub fn open_read(&self, rel: &[u8]) -> io::Result<File> {
        let fd = self.open_entry(rel, OFlags::RDONLY | OFlags::NONBLOCK)?;
        Ok(File::from(fd))
    }

    pub fn read_all(&self, rel: &[u8]) -> io::Result<Vec<u8>> {
        let mut f = self.open_read(rel)?;
        let mut v = Vec::new();
        io::Read::read_to_end(&mut f, &mut v)?;
        Ok(v)
    }

    pub fn readlink(&self, rel: &[u8]) -> io::Result<Vec<u8>> {
        let (dir, name) = self.parent(rel)?;
        Ok(rustix::fs::readlinkat(&dir, name, Vec::new())?.into_bytes())
    }

    fn xattr_fd(&self, rel: &[u8]) -> io::Result<Option<OwnedFd>> {
        match self.lstat(rel)? {
            Some(st) => self.xattr_fd_of(rel, &st),
            None => Ok(None),
        }
    }

    pub fn xattr_fd_of(&self, rel: &[u8], st: &Stat) -> io::Result<Option<OwnedFd>> {
        let flags = match st.ftype() {
            FType::Dir => OFlags::RDONLY | OFlags::DIRECTORY,
            FType::Reg => OFlags::RDONLY | OFlags::NONBLOCK,
            _ => return Ok(None),
        };
        self.open_entry(rel, flags).map(Some)
    }

    pub fn user_xattrs(&self, rel: &[u8]) -> io::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let Some(fd) = self.xattr_fd(rel)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for k in flistxattr(&fd)? {
            if k.starts_with(b"user.") {
                let ks = String::from_utf8_lossy(&k).into_owned();
                if let Some(v) = fgetxattr(&fd, &ks)? {
                    out.push((k, v));
                }
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn unlink(&self, rel: &[u8], dir: bool) -> io::Result<()> {
        let (d, name) = self.parent(rel)?;
        let flags = if dir { AtFlags::REMOVEDIR } else { AtFlags::empty() };
        Ok(rustix::fs::unlinkat(&d, name, flags)?)
    }

    pub fn mkdir(&self, rel: &[u8], mode: u32) -> io::Result<()> {
        let (d, name) = self.parent(rel)?;
        Ok(rustix::fs::mkdirat(&d, name, Mode::from_raw_mode(mode))?)
    }

    pub fn symlink(&self, rel: &[u8], target: &[u8]) -> io::Result<()> {
        let (d, name) = self.parent(rel)?;
        Ok(rustix::fs::symlinkat(target, &d, name)?)
    }

    pub fn mkfifo(&self, rel: &[u8], mode: u32) -> io::Result<()> {
        let (d, name) = self.parent(rel)?;
        Ok(rustix::fs::mknodat(&d, name, FileType::Fifo, Mode::from_raw_mode(mode), 0)?)
    }

    pub fn create_temp(&self, rel: &[u8]) -> io::Result<(File, Vec<u8>)> {
        let d = self.dir(path::parent(rel))?;
        for i in 0..100u32 {
            let tmp = format!(".acfs-tmp-{}-{}-{}", std::process::id(), nanos(), i).into_bytes();
            match rustix::fs::openat(&d, &tmp, OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW, Mode::from_raw_mode(0o600))
            {
                Ok(fd) => return Ok((File::from(fd), tmp)),
                Err(Errno::EXIST) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(Errno::EXIST.into())
    }

    pub fn rename_into(&self, rel: &[u8], from_name: &[u8]) -> io::Result<()> {
        let (d, name) = self.parent(rel)?;
        Ok(rustix::fs::renameat(&d, from_name, &d, name)?)
    }

    pub fn rename_noreplace(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        let (fd, fname) = self.parent(from)?;
        let (td, tname) = self.parent(to)?;
        Ok(rustix::fs::renameat_with(&fd, fname, &td, tname, RenameFlags::NOREPLACE)?)
    }

    pub fn lchown(&self, rel: &[u8], uid: u32, gid: u32) -> io::Result<()> {
        let (u, g) = (Some(Uid::from_raw(uid)), Some(Gid::from_raw(gid)));
        if rel.is_empty() {
            return Ok(rustix::fs::fchown(&self.root, u, g)?);
        }
        let (d, name) = self.parent(rel)?;
        Ok(rustix::fs::chownat(&d, name, u, g, AtFlags::SYMLINK_NOFOLLOW)?)
    }

    pub fn chmod(&self, rel: &[u8], mode: u32) -> io::Result<()> {
        let fd = self.open_entry(rel, OFlags::RDONLY | OFlags::NONBLOCK)?;
        Ok(rustix::fs::fchmod(&fd, Mode::from_raw_mode(mode))?)
    }

    pub fn set_times(&self, rel: &[u8], atime: (i64, i64), mtime: (i64, i64)) -> io::Result<()> {
        let ts = timestamps(atime, mtime);
        if rel.is_empty() {
            return Ok(rustix::fs::futimens(&self.root, &ts)?);
        }
        let (d, name) = self.parent(rel)?;
        Ok(rustix::fs::utimensat(&d, name, &ts, AtFlags::SYMLINK_NOFOLLOW)?)
    }

    pub fn set_user_xattrs(&self, rel: &[u8], want: &[(Vec<u8>, Vec<u8>)]) -> io::Result<()> {
        let Some(fd) = self.xattr_fd(rel)? else {
            return Ok(());
        };
        set_user_xattrs_fd(&fd, want)
    }
}

pub fn set_user_xattrs_fd(fd: &impl AsFd, want: &[(Vec<u8>, Vec<u8>)]) -> io::Result<()> {
    for k in flistxattr(fd)? {
        if k.starts_with(b"user.") && !want.iter().any(|(w, _)| *w == k) {
            rustix::fs::fremovexattr(fd, &k)?;
        }
    }
    for (k, v) in want {
        rustix::fs::fsetxattr(fd, k, v, XattrFlags::empty())?;
    }
    Ok(())
}

fn xattr_buf(absent: &[Errno], mut f: impl FnMut(&mut [u8]) -> rustix::io::Result<usize>) -> io::Result<Option<Vec<u8>>> {
    let mut buf = vec![0u8; 4096];
    loop {
        match f(&mut buf) {
            Ok(n) => {
                buf.truncate(n);
                return Ok(Some(buf));
            }
            Err(e) if absent.contains(&e) => return Ok(None),
            Err(Errno::RANGE) if buf.len() < 1 << 20 => buf.resize(buf.len() * 4, 0),
            Err(e) => return Err(e.into()),
        }
    }
}

pub fn fgetxattr(fd: &impl AsFd, key: &str) -> io::Result<Option<Vec<u8>>> {
    xattr_buf(&[Errno::NODATA, Errno::OPNOTSUPP], |b| rustix::fs::fgetxattr(fd, key, b))
}

fn xattr_names(buf: Option<Vec<u8>>) -> Vec<Vec<u8>> {
    buf.unwrap_or_default().split(|&c| c == 0).filter(|s| !s.is_empty()).map(|s| s.to_vec()).collect()
}

pub fn flistxattr(fd: &impl AsFd) -> io::Result<Vec<Vec<u8>>> {
    Ok(xattr_names(xattr_buf(&[Errno::OPNOTSUPP], |b| rustix::fs::flistxattr(fd, b))?))
}

pub fn llistxattr(p: &Path) -> io::Result<Vec<Vec<u8>>> {
    Ok(xattr_names(xattr_buf(&[Errno::OPNOTSUPP], |b| rustix::fs::llistxattr(p, b))?))
}

pub fn nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}
