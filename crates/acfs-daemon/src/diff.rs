use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use acfs_core::merge::{ChangeKind, FType};
use acfs_core::path::{self, PathName, RelPath};
use acfs_core::results::Change;
use acfs_core::rules::Rules;
use anyhow::{Result, bail};
use sha2::{Digest, Sha256};

use crate::store::SnapTrees;
use crate::tree::{Stat, Tree, fgetxattr};

pub struct Side<'a> {
    pub upper: &'a Tree,
    pub view: &'a Tree,
    pub dir: &'a Path,
}

#[derive(Default, Debug)]
pub struct Changes {
    pub kinds: BTreeMap<RelPath, ChangeKind>,
    pub renames: BTreeMap<RelPath, RelPath>,
}

#[derive(PartialEq, Eq, Debug, Clone)]
enum Source {
    Same,
    Whiteout,
    NonDir,
    Opaque,
    Redirect(Vec<u8>),
}

fn dir_source(upper: &Tree, rel: &[u8]) -> Result<Source> {
    let Some(st) = upper.lstat(rel)? else { return Ok(Source::Same) };
    if st.is_whiteout() {
        return Ok(Source::Whiteout);
    }
    if !st.is_dir() {
        return Ok(Source::NonDir);
    }
    let Some(fd) = upper.xattr_fd_of(rel, &st)? else { return Ok(Source::NonDir) };
    if fgetxattr(&fd, "trusted.overlay.opaque")?.as_deref() == Some(b"y") {
        return Ok(Source::Opaque);
    }
    Ok(fgetxattr(&fd, "trusted.overlay.redirect")?.map_or(Source::Same, Source::Redirect))
}

pub fn sha256(mut r: impl Read) -> std::io::Result<Vec<u8>> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 << 10];
    loop {
        match r.read(&mut buf) {
            Ok(0) => return Ok(h.finalize().to_vec()),
            Ok(n) => h.update(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

fn same_entry(a: &Side, b: &Side, rel: &[u8], sa: &Stat, sb: &Stat, same_base: bool) -> Result<bool> {
    let (pa, pb) = if sa.ftype() != FType::Dir { (a.upper.lstat(rel)?, b.upper.lstat(rel)?) } else { (None, None) };
    if let (Some(pa), Some(pb)) = (&pa, &pb) {
        let key = |s: &Stat| (s.ino, s.size, s.mtime, s.ctime, s.mode, s.uid, s.gid);
        if !pa.is_whiteout() && key(pa) == key(pb) {
            return Ok(true);
        }
    }
    if sa.meta() != sb.meta() {
        return Ok(false);
    }
    if a.view.user_xattrs(rel)? != b.view.user_xattrs(rel)? {
        return Ok(false);
    }
    match sa.ftype() {
        FType::Dir => Ok(true),
        FType::Symlink => Ok(a.view.readlink(rel)? == b.view.readlink(rel)?),
        FType::Reg => {
            if sa.size != sb.size {
                return Ok(false);
            }
            if same_base && sa.ino == sb.ino && sa.mtime == sb.mtime && pa.is_none() && pb.is_none() {
                return Ok(true);
            }
            Ok(sha256(a.view.open_read(rel)?)? == sha256(b.view.open_read(rel)?)?)
        }
        _ => Ok(sa.rdev == sb.rdev),
    }
}

fn subtree(view: &Tree, rel: &[u8], out: &mut Vec<RelPath>) -> Result<()> {
    out.push(rel.to_vec());
    if view.lstat(rel)?.is_some_and(|s| s.is_dir()) {
        for n in view.list(rel)? {
            subtree(view, &path::join(rel, &n), out)?;
        }
    }
    Ok(())
}

struct Walk<'a> {
    a: &'a Side<'a>,
    b: &'a Side<'a>,
    same_base: bool,
    out: Changes,
}

impl Walk<'_> {
    fn mark(&mut self, view: &Tree, rel: &[u8], kind: ChangeKind) -> Result<()> {
        let mut v = Vec::new();
        subtree(view, rel, &mut v)?;
        for p in v {
            self.out.kinds.insert(p, kind);
        }
        Ok(())
    }

    fn entry(&mut self, r: &[u8], full: bool, recurse: bool) -> Result<bool> {
        let sa = self.a.view.lstat(r)?;
        let sb = self.b.view.lstat(r)?;
        match (sa, sb) {
            (None, None) => Ok(true),
            (None, Some(sb)) => {
                self.mark(self.b.view, r, ChangeKind::Add)?;
                if sb.is_dir()
                    && let Source::Redirect(from) = dir_source(self.b.upper, r)?
                {
                    self.out.renames.insert(r.to_vec(), path::resolve_redirect(r, &from));
                }
                Ok(true)
            }
            (Some(_), None) => {
                self.mark(self.a.view, r, ChangeKind::Delete)?;
                Ok(true)
            }
            (Some(sa), Some(sb)) if sa.ftype() != sb.ftype() => {
                self.mark(self.a.view, r, ChangeKind::Delete)?;
                self.mark(self.b.view, r, ChangeKind::Add)?;
                self.out.kinds.insert(r.to_vec(), ChangeKind::Modify);
                Ok(true)
            }
            (Some(sa), Some(sb)) => {
                if !same_entry(self.a, self.b, r, &sa, &sb, self.same_base)? {
                    self.out.kinds.insert(r.to_vec(), ChangeKind::Modify);
                }
                if !sb.is_dir() {
                    return Ok(true);
                }
                let src_a = dir_source(self.a.upper, r)?;
                let src_b = dir_source(self.b.upper, r)?;
                if let Source::Redirect(from) = &src_b
                    && src_a != src_b
                {
                    self.out.renames.insert(r.to_vec(), path::resolve_redirect(r, from));
                }
                let expand = full || src_a != src_b;
                if recurse || expand {
                    self.walk(r, expand)?;
                }
                Ok(recurse || expand)
            }
        }
    }

    fn walk(&mut self, rel: &[u8], full: bool) -> Result<()> {
        let mut names = self.a.upper.list(rel)?;
        names.extend(self.b.upper.list(rel)?);
        if full {
            names.extend(self.a.view.list(rel)?);
            names.extend(self.b.view.list(rel)?);
        }
        names.sort();
        names.dedup();
        for n in names {
            self.entry(&path::join(rel, &n), full, true)?;
        }
        Ok(())
    }

    fn root(&mut self) -> Result<()> {
        if let (Some(ra), Some(rb)) = (self.a.view.lstat(b"")?, self.b.view.lstat(b"")?)
            && (ra.meta() != rb.meta() || self.a.view.user_xattrs(b"")? != self.b.view.user_xattrs(b"")?)
        {
            self.out.kinds.insert(Vec::new(), ChangeKind::Modify);
        }
        Ok(())
    }
}

fn v1(a: &Side, b: &Side, same_base: bool, full: bool) -> Result<Changes> {
    let mut w = Walk { a, b, same_base, out: Changes::default() };
    w.root()?;
    w.walk(b"", full || !same_base)?;
    Ok(w.out)
}

fn v2(a: &Side, b: &Side) -> Result<Changes> {
    let touched = crate::send::touched_upper(a.dir, b.dir)?;
    let mut w = Walk { a, b, same_base: true, out: Changes::default() };
    w.root()?;
    let mut done: HashSet<RelPath> = HashSet::new();
    for r in touched.iter().filter(|r| !r.is_empty()) {
        if path::ancestors(r).iter().any(|a| done.contains(*a)) || done.contains(r) {
            continue;
        }
        if w.entry(r, false, false)? {
            done.insert(r.clone());
        }
    }
    Ok(w.out)
}

pub fn diff(a: Option<&Side>, b: &Side, same_base: bool, full: bool) -> Result<Changes> {
    let Some(a) = a else {
        let mut v = Vec::new();
        subtree(b.view, b"", &mut v)?;
        return Ok(Changes { kinds: v.into_iter().skip(1).map(|p| (p, ChangeKind::Add)).collect(), ..Default::default() });
    };
    if !same_base || full || std::env::var_os("ACFS_DIFF_V1").is_some() {
        return v1(a, b, same_base, full);
    }
    let fast = match v2(a, b) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("acfs-daemon: diff v2 of {} and {}: {e:#}; walking the uppers", a.dir.display(), b.dir.display());
            return v1(a, b, true, false);
        }
    };
    if std::env::var_os("ACFS_DIFF_CHECK").is_some() {
        let slow = v1(a, b, true, false)?;
        if slow.kinds != fast.kinds || slow.renames != fast.renames {
            let show = |c: &Changes| format!("{:?} {:?}", c.kinds.iter().map(|(p, k)| (String::from_utf8_lossy(p), k)).collect::<Vec<_>>(), c.renames);
            bail!("diff v2 disagrees with v1 for {} -> {}:\nv1 {}\nv2 {}", a.dir.display(), b.dir.display(), show(&slow), show(&fast));
        }
    }
    Ok(fast)
}

pub fn snaps(a: Option<&SnapTrees>, b: &SnapTrees, full: bool) -> Result<Changes> {
    match a {
        None => diff(None, &b.side(), true, true),
        Some(a) => diff(Some(&a.side()), &b.side(), a.base == b.base, full),
    }
}

pub fn filtered(a: &SnapTrees, b: &SnapTrees) -> Result<Changes> {
    Ok(snaps(Some(a), b, false)?.without_ignored(&crate::merging::rules_of(&b.view), Some(&a.view), &b.view))
}

impl Changes {
    pub fn without_ignored(mut self, rules: &Rules, a: Option<&Tree>, b: &Tree) -> Changes {
        if rules.is_empty() {
            return self;
        }
        let is_dir = |t: Option<&Tree>, p: &[u8]| t.and_then(|t| t.lstat(p).ok().flatten()).is_some_and(|s| s.is_dir());
        self.kinds.retain(|p, k| !rules.ignored(p, if *k == ChangeKind::Delete { is_dir(a, p) } else { is_dir(Some(b), p) }));
        self.renames.retain(|p, _| self.kinds.contains_key(p));
        self
    }

    pub fn with_identity_renames(mut self, a: &Side, b: &Side) -> Changes {
        let mut gone: HashMap<u64, Option<RelPath>> = HashMap::new();
        for (p, k) in &self.kinds {
            if *k != ChangeKind::Add
                && let Some(id) = ident(a, p)
            {
                gone.entry(id).and_modify(|e| *e = None).or_insert_with(|| Some(p.clone()));
            }
        }
        let mut found = BTreeMap::new();
        for (p, k) in &self.kinds {
            if *k == ChangeKind::Add
                && let Some(Some(from)) = ident(b, p).and_then(|id| gone.get(&id))
                && a.view.lstat(from).ok().flatten().map(|s| s.ftype()) == b.view.lstat(p).ok().flatten().map(|s| s.ftype())
            {
                found.insert(p.clone(), from.clone());
            }
        }
        for (p, from) in &found {
            let implied = found.get(path::parent(p)).is_some_and(|pf| path::parent(from) == pf.as_slice() && path::file_name(from) == path::file_name(p));
            if !implied && !self.renames.contains_key(p) {
                self.renames.insert(p.clone(), from.clone());
            }
        }
        self
    }
}

pub fn ident(side: &Side, p: &[u8]) -> Option<u64> {
    match side.upper.lstat(p).ok()? {
        Some(st) if !st.is_whiteout() => {
            if let Ok(Some(fd)) = side.upper.xattr_fd_of(p, &st)
                && let Ok(Some(fh)) = fgetxattr(&fd, "trusted.overlay.origin")
            {
                return origin_ino(&fh);
            }
            Some(st.ino)
        }
        Some(_) => None,
        None => side.view.lstat(p).ok()?.map(|s| s.ino),
    }
}

fn origin_ino(fh: &[u8]) -> Option<u64> {
    const BTRFS_FID_TYPES: [u8; 3] = [0x4d, 0x4e, 0x4f];
    if fh.len() < 29 || fh[1] != 0xfb || !BTRFS_FID_TYPES.contains(&fh[4]) {
        return None;
    }
    Some(u64::from_le_bytes(fh[21..29].try_into().ok()?))
}

pub fn to_api(c: &Changes) -> Vec<Change> {
    c.kinds
        .iter()
        .map(|(p, k)| match c.renames.get(p) {
            Some(from) if *k == ChangeKind::Add => Change { path: PathName::new(p), kind: ChangeKind::Rename, from: Some(PathName::new(from)) },
            _ => Change { path: PathName::new(p), kind: *k, from: None },
        })
        .collect()
}
