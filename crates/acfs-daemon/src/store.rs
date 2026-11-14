use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, chown};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::Connection;

use crate::daemon::{Daemon, snap_get};
use crate::diff;
use crate::sys;
use crate::tree::Tree;

pub fn layers(subvol: &Path) -> Result<Vec<PathBuf>> {
    let mut n: Vec<u32> = Vec::new();
    for e in fs::read_dir(subvol.join("l")).with_context(|| format!("read layers of {}", subvol.display()))? {
        let name = e?.file_name();
        let s = name.to_string_lossy();
        n.push(s.parse().map_err(|_| anyhow!("unexpected layer {s:?} in {}", subvol.display()))?);
    }
    n.sort_unstable_by(|a, b| b.cmp(a));
    if n.is_empty() {
        bail!("{} has no layers", subvol.display());
    }
    Ok(n.into_iter().map(|i| subvol.join("l").join(i.to_string())).collect())
}

pub fn reset_work(subvol: &Path) -> Result<()> {
    let w = subvol.join("work");
    if w.exists() {
        fs::remove_dir_all(&w).with_context(|| format!("clear {}", w.display()))?;
    }
    fs::create_dir(&w)?;
    fs::set_permissions(&w, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub fn copy_dir_attrs(from: &Path, to: &Path) -> Result<()> {
    let m = fs::symlink_metadata(from)?;
    chown(to, Some(m.uid()), Some(m.gid()))?;
    fs::set_permissions(to, fs::Permissions::from_mode(m.mode() & 0o7777))?;
    let t = Tree::open(to)?;
    t.set_times(b"", (m.atime(), m.atime_nsec()), (m.mtime(), m.mtime_nsec()))?;
    Ok(())
}

impl Daemon {
    pub fn fork_subvol(&self, snap: &str, branch: &str) -> Result<()> {
        let dst = self.paths.branch(branch);
        sys::snapshot(&self.paths.snap(snap), &dst, false).with_context(|| format!("snapshot {snap} -> branch {branch}"))?;
        if let Err(e) = reset_work(&dst) {
            let _ = sys::subvol_delete(&dst);
            return Err(e);
        }
        Ok(())
    }

    pub fn snap_branch(&self, branch: &str, snap: &str) -> Result<()> {
        sys::snapshot(&self.paths.branch(branch), &self.paths.snap(snap), true).with_context(|| format!("snapshot branch {branch}"))
    }

    pub fn delete_subvol(&self, p: &Path) -> Result<()> {
        if !p.exists() {
            return Ok(());
        }
        sys::subvol_delete(p).with_context(|| format!("delete subvolume {}", p.display()))
    }

    pub fn mount_branch(&self, branch: &str) -> Result<PathBuf> {
        let target = self.paths.mnt(branch);
        if sys::is_mountpoint(&target) {
            return Ok(target);
        }
        fs::create_dir_all(&target)?;
        self.mount_branch_at(branch, &target)?;
        Ok(target)
    }

    pub fn mount_branch_at(&self, branch: &str, target: &Path) -> Result<()> {
        let dir = self.paths.branch(branch);
        if !dir.join("upper").is_dir() {
            bail!("branch {branch} has no subvolume");
        }
        let fd = sys::overlay(&layers(&dir)?, Some((dir.join("upper"), dir.join("work")))).with_context(|| format!("mount branch {branch}"))?;
        sys::move_mount(&fd, target).with_context(|| format!("attach {}", target.display()))
    }

    pub fn unmount_branch(&self, branch: &str) -> Result<()> {
        let target = self.paths.mnt(branch);
        sys::unmount_and_remove(&target).with_context(|| format!("unmount {}", target.display()))
    }

    pub fn reset_branch(&self, branch: &str, snap: &str) -> Result<()> {
        self.unmount_branch(branch)?;
        self.delete_subvol(&self.paths.branch(branch))?;
        self.fork_subvol(snap, branch)
    }

    pub fn drop_branch(&self, c: &Connection, branch: &str) -> Result<()> {
        self.unmount_branch(branch)?;
        self.delete_subvol(&self.paths.branch(branch))?;
        c.execute("UPDATE branches SET alive = 0 WHERE id = ?1", [branch])?;
        Ok(())
    }

    pub fn view(&self, snap: &str) -> Result<View> {
        let dir = self.paths.snap(snap);
        let mut lowers = vec![dir.join("upper")];
        lowers.extend(layers(&dir)?);
        let fd = sys::overlay(&lowers, None).with_context(|| format!("mount view of {snap}"))?;
        let path = self.paths.views().join(format!("{snap}-{}", crate::tree::nanos()));
        fs::create_dir_all(&path)?;
        if let Err(e) = sys::move_mount(&fd, &path) {
            let _ = fs::remove_dir(&path);
            return Err(e).context("attach view");
        }
        Ok(View { path })
    }
}

pub struct View {
    pub path: PathBuf,
}

pub struct SnapTrees {
    pub upper: Tree,
    pub view: Tree,
    pub base: String,
    pub dir: PathBuf,
    _mount: View,
}

impl SnapTrees {
    pub fn open(d: &Daemon, c: &Connection, snap: &str) -> Result<SnapTrees> {
        let s = snap_get(c, snap)?;
        if !s.present {
            bail!("snapshot {snap} no longer exists");
        }
        Self::at(d, snap, s.base)
    }

    pub fn at(d: &Daemon, snap: &str, base: String) -> Result<SnapTrees> {
        let mount = d.view(snap)?;
        let dir = d.paths.snap(snap);
        Ok(SnapTrees { upper: Tree::open_ro(&dir.join("upper"))?, view: Tree::open_ro(&mount.path)?, base, dir, _mount: mount })
    }

    pub fn side(&self) -> diff::Side<'_> {
        diff::Side { upper: &self.upper, view: &self.view, dir: &self.dir }
    }
}

pub struct Undo<'a> {
    d: &'a Daemon,
    steps: Vec<Box<dyn FnOnce() + 'a>>,
}

impl<'a> Undo<'a> {
    pub fn new(d: &'a Daemon) -> Undo<'a> {
        Undo { d, steps: Vec::new() }
    }

    pub fn defer(&mut self, f: impl FnOnce() + 'a) {
        self.steps.push(Box::new(f));
    }

    pub fn subvol(&mut self, p: PathBuf) {
        let d = self.d;
        self.defer(move || {
            let _ = d.delete_subvol(&p);
        });
    }

    pub fn mount(&mut self, p: PathBuf) {
        self.defer(move || {
            let _ = sys::unmount_and_remove(&p);
        });
    }

    pub fn commit(mut self) {
        self.steps.clear();
    }
}

impl Drop for Undo<'_> {
    fn drop(&mut self) {
        while let Some(f) = self.steps.pop() {
            f();
        }
    }
}

impl Drop for View {
    fn drop(&mut self) {
        static REAPER: OnceLock<Sender<PathBuf>> = OnceLock::new();
        let tx = REAPER.get_or_init(|| {
            let (tx, rx) = channel::<PathBuf>();
            std::thread::spawn(move || {
                for p in rx {
                    if let Err(e) = sys::umount(&p, true).and_then(|()| fs::remove_dir(&p)) {
                        eprintln!("acfs-daemon: unmount view {}: {e}", p.display());
                    }
                }
            });
            tx
        });
        let _ = tx.send(std::mem::take(&mut self.path));
    }
}
