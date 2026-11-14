use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use acfs_core::merge::{FType, Meta};
use acfs_core::path::{self, PathName, RelPath, b64_decode, b64_encode};
use acfs_core::proto::{FsAt, FsEdit, FsList, FsOp, FsRead, FsStat};
use acfs_core::results::{FsEditStatus, FsEdited, FsEntry, FsFile, FsKind, FsListing};
use anyhow::Result;
use rusqlite::Connection;

use super::history::resolve;
use super::live;
use crate::daemon::*;
use crate::diff;
use crate::merging;
use crate::store::SnapTrees;
use crate::tree::{Stat, Tree};

const MAX_READ: u64 = 64 << 20;

fn rel(p: &PathName) -> Result<RelPath> {
    let b = p.to_bytes().ok_or_else(|| invalid("bad path encoding"))?;
    let b = b.strip_suffix(b"/").map(<[u8]>::to_vec).unwrap_or(b);
    if b.is_empty() {
        return Ok(b);
    }
    if !path::is_valid(&b) {
        return Err(invalid(format!("invalid path {:?}", String::from_utf8_lossy(&b))));
    }
    Ok(b)
}

struct Opened {
    ws: Ws,
    snap: Option<String>,
    root: Root,
}

enum Root {
    Dir(Tree),
    View(SnapTrees),
}

impl Opened {
    fn tree(&self) -> &Tree {
        match &self.root {
            Root::Dir(t) => t,
            Root::View(v) => &v.view,
        }
    }
}

fn open_at(d: &Daemon, uid: u32, a: &FsAt) -> Result<Opened> {
    let c = d.db()?;
    let ws = ws_for(&c, uid, &a.ws)?;
    let snap = match &a.at {
        Some(at) => {
            let snaps = resolve(&c, at)?;
            check_snaps(&c, uid, at, &snaps)?;
            let s = snaps.into_iter().find(|s| s.ws == ws.id).ok_or_else(|| invalid(format!("{at} has no snapshot of {}", ws.name)))?;
            Some(s.id)
        }
        None if ws.live => None,
        None => Some(branch_get(&c, &ws.main_branch)?.head_snap),
    };
    match snap {
        None => Ok(Opened { root: Root::Dir(Tree::open(Path::new(&ws.host_path))?), ws, snap: None }),
        Some(s) => Ok(Opened { root: Root::View(SnapTrees::open(d, &c, &s)?), ws, snap: Some(s) }),
    }
}

fn kind(st: &Stat) -> FsKind {
    match st.ftype() {
        FType::Reg => FsKind::File,
        FType::Dir => FsKind::Dir,
        FType::Symlink => FsKind::Symlink,
        FType::Fifo => FsKind::Fifo,
        FType::Other => FsKind::Other,
    }
}

fn entry(p: &[u8], st: &Stat) -> FsEntry {
    FsEntry {
        path: PathName::new(p),
        kind: kind(st),
        size: st.size,
        mode: st.mode & 0o7777,
        uid: st.uid,
        gid: st.gid,
        mtime: st.mtime.0 * 1000 + st.mtime.1 / 1_000_000,
    }
}

fn stat_of(t: &Tree, p: &[u8]) -> Result<Stat> {
    t.lstat(p)?.ok_or_else(|| not_found("path", &String::from_utf8_lossy(p)))
}

pub fn read(d: &Daemon, uid: u32, a: FsRead) -> Result<FsFile> {
    let o = open_at(d, uid, &a.at)?;
    let p = rel(&a.path)?;
    let st = stat_of(o.tree(), &p)?;
    let mut out = FsFile { ws: o.ws.name.clone(), snap: o.snap.clone(), entry: entry(&p, &st), content: None, target: None };
    match kind(&st) {
        FsKind::File => {
            let mut f = o.tree().open_read(&p)?;
            let offset = a.offset.unwrap_or(0);
            let want = a.length.unwrap_or(st.size.saturating_sub(offset));
            if want > MAX_READ {
                return Err(invalid(format!("reads are limited to {MAX_READ} bytes; pass offset and length")));
            }
            f.seek(SeekFrom::Start(offset))?;
            let mut buf = Vec::new();
            f.take(want).read_to_end(&mut buf)?;
            out.content = Some(b64_encode(&buf));
        }
        FsKind::Symlink => out.target = Some(PathName::new(&o.tree().readlink(&p)?)),
        FsKind::Dir => return Err(invalid(format!("{} is a directory", String::from_utf8_lossy(&p)))),
        _ => {}
    }
    Ok(out)
}

pub fn list(d: &Daemon, uid: u32, a: FsList) -> Result<FsListing> {
    let o = open_at(d, uid, &a.at)?;
    let p = a.path.as_ref().map(rel).transpose()?.unwrap_or_default();
    if !stat_of(o.tree(), &p)?.is_dir() {
        return Err(invalid(format!("{} is not a directory", String::from_utf8_lossy(&p))));
    }
    let mut entries = Vec::new();
    for n in o.tree().list(&p)? {
        if let Some(st) = o.tree().lstat(&path::join(&p, &n))? {
            entries.push(entry(&n, &st));
        }
    }
    Ok(FsListing { ws: o.ws.name.clone(), snap: o.snap.clone(), entries })
}

pub fn stat(d: &Daemon, uid: u32, a: FsStat) -> Result<FsEntry> {
    let o = open_at(d, uid, &a.at)?;
    let p = rel(&a.path)?;
    Ok(entry(&p, &stat_of(o.tree(), &p)?))
}

enum Op {
    Write(RelPath, Vec<u8>, Option<u32>),
    Mkdir(RelPath, Option<u32>),
    Delete(RelPath),
    Rename(RelPath, RelPath),
    Symlink(RelPath, Vec<u8>),
    Chmod(RelPath, u32),
}

impl Op {
    fn paths(&self) -> Vec<&RelPath> {
        match self {
            Op::Write(p, ..) | Op::Mkdir(p, _) | Op::Delete(p) | Op::Symlink(p, _) | Op::Chmod(p, _) => vec![p],
            Op::Rename(a, b) => vec![a, b],
        }
    }
}

fn decode(ops: &[FsOp]) -> Result<Vec<Op>> {
    let nonroot = |p: &PathName| -> Result<RelPath> {
        let r = rel(p)?;
        if r.is_empty() {
            return Err(invalid("the workspace root cannot be changed this way"));
        }
        Ok(r)
    };
    let mode = |m: Option<u32>| -> Result<Option<u32>> {
        match m {
            Some(m) if m & !0o7777 != 0 => Err(invalid(format!("bad mode {m:o}"))),
            m => Ok(m),
        }
    };
    ops.iter()
        .map(|o| {
            Ok(match o {
                FsOp::Write { path, content, mode: m } => {
                    Op::Write(nonroot(path)?, b64_decode(content).ok_or_else(|| invalid("content is not base64"))?, mode(*m)?)
                }
                FsOp::Mkdir { path, mode: m } => Op::Mkdir(nonroot(path)?, mode(*m)?),
                FsOp::Delete { path } => Op::Delete(nonroot(path)?),
                FsOp::Rename { from, to } => Op::Rename(nonroot(from)?, nonroot(to)?),
                FsOp::Symlink { path, target } => Op::Symlink(nonroot(path)?, target.to_bytes().ok_or_else(|| invalid("bad target encoding"))?),
                FsOp::Chmod { path, mode: m } => Op::Chmod(nonroot(path)?, mode(Some(*m))?.unwrap()),
            })
        })
        .collect()
}

fn parents(t: &Tree, p: &[u8], ws: &Ws) -> Result<()> {
    for anc in path::ancestors(p) {
        match t.lstat(anc)? {
            Some(st) if st.is_dir() => {}
            Some(_) => return Err(invalid(format!("{} is not a directory", String::from_utf8_lossy(anc)))),
            None => {
                t.mkdir(anc, 0o755)?;
                t.lchown(anc, ws.uid, ws.gid)?;
            }
        }
    }
    Ok(())
}

fn run_op(t: &Tree, ws: &Ws, op: &Op) -> Result<()> {
    let missing = |p: &[u8]| not_found("path", &String::from_utf8_lossy(p));
    match op {
        Op::Write(p, data, mode) => {
            parents(t, p, ws)?;
            let old = t.lstat(p)?;
            if old.is_some_and(|s| s.is_dir()) {
                return Err(invalid(format!("{} is a directory", String::from_utf8_lossy(p))));
            }
            let (meta, xattrs) = match old.filter(|s| s.ftype() == FType::Reg) {
                Some(s) => (s.meta(), t.user_xattrs(p)?),
                None => (Meta { mode: 0o644, uid: ws.uid, gid: ws.gid }, Vec::new()),
            };
            let meta = Meta { mode: mode.unwrap_or(meta.mode), ..meta };
            merging::write_file(t, p, &mut &data[..], meta, None, &xattrs, true)
        }
        Op::Mkdir(p, mode) => {
            parents(t, p, ws)?;
            if t.lstat(p)?.is_some() {
                return Err(api("exists", format!("{} exists", String::from_utf8_lossy(p))));
            }
            t.mkdir(p, mode.unwrap_or(0o755))?;
            t.lchown(p, ws.uid, ws.gid)?;
            t.chmod(p, mode.unwrap_or(0o755))?;
            Ok(())
        }
        Op::Delete(p) => {
            t.lstat(p)?.ok_or_else(|| missing(p))?;
            merging::remove(t, p)
        }
        Op::Rename(a, b) => {
            t.lstat(a)?.ok_or_else(|| missing(a))?;
            if path::is_under(b, a) {
                return Err(invalid("cannot move a directory into itself"));
            }
            parents(t, b, ws)?;
            t.rename_noreplace(a, b)
                .map_err(|e| if t.lstat(b).ok().flatten().is_some() { api("exists", format!("{} exists", String::from_utf8_lossy(b))) } else { e.into() })
        }
        Op::Symlink(p, target) => {
            parents(t, p, ws)?;
            t.symlink(p, target)
                .map_err(|e| if t.lstat(p).ok().flatten().is_some() { api("exists", format!("{} exists", String::from_utf8_lossy(p))) } else { e.into() })?;
            t.lchown(p, ws.uid, ws.gid)?;
            Ok(())
        }
        Op::Chmod(p, mode) => {
            let st = t.lstat(p)?.ok_or_else(|| missing(p))?;
            if st.ftype() == FType::Symlink {
                return Err(invalid("symlinks have no mode"));
            }
            Ok(t.chmod(p, *mode)?)
        }
    }
}

fn touched_since(d: &Daemon, c: &Connection, base: &str, head: &str, ops: &[Op]) -> Result<Vec<RelPath>> {
    if base == head {
        return Ok(Vec::new());
    }
    let a = SnapTrees::open(d, c, base)?;
    let b = SnapTrees::open(d, c, head)?;
    let ch = diff::snaps(Some(&a), &b, false)?;
    let mine: BTreeSet<&RelPath> = ops.iter().flat_map(|o| o.paths()).collect();
    Ok(ch.kinds.keys().filter(|q| mine.iter().any(|p| *q == *p || path::is_under(q, p) || path::is_under(p, q))).cloned().collect())
}

pub fn edit(d: &Daemon, uid: u32, a: FsEdit) -> Result<FsEdited> {
    if a.ops.is_empty() {
        return Err(invalid("no ops"));
    }
    let ops = decode(&a.ops)?;
    let c0 = d.db()?;
    let ws = ws_for(&c0, uid, &a.ws)?;
    let base = match &a.base {
        Some(b) => {
            let snaps = resolve(&c0, b)?;
            check_snaps(&c0, uid, b, &snaps)?;
            Some(snaps.into_iter().find(|s| s.ws == ws.id).ok_or_else(|| invalid(format!("{b} has no snapshot of {}", ws.name)))?.id)
        }
        None => None,
    };
    drop(c0);
    let _g = d.locks.lock(ws.keys());
    let mut c = d.db()?;
    let ws = ws_get(&c, &ws.id)?;
    if !ws.live {
        return Err(bad_state(format!("{} is detached", ws.name)));
    }
    let target = branch_get(&c, &ws.main_branch)?;
    live::sync(d, &mut c, &ws, "before fs.edit")?;
    let head = branch_get(&c, &target.id)?.head_snap;
    let mut out = FsEdited { ws: ws.name.clone(), status: FsEditStatus::Applied, changeset: None, snap: None, conflicts: Vec::new(), conflict: false };
    if let Some(b) = &base {
        let changed = touched_since(d, &c, b, &head, &ops)?;
        if !changed.is_empty() {
            out.status = FsEditStatus::Conflict;
            out.conflict = true;
            out.conflicts = merging::path_names(&changed);
            return Ok(out);
        }
    }
    let dir = PathBuf::from(&ws.host_path);
    journal_begin(&c, &target.id, &head, "fs.edit")?;
    let applied = (|| -> Result<()> {
        let t = Tree::open(&dir)?;
        for op in &ops {
            run_op(&t, &ws, op)?;
        }
        Ok(())
    })();
    if let Err(e) = applied {
        let _ = crate::recover::restore_branch(d, &mut c, &target.id);
        return Err(e);
    }
    let author = a.author.as_deref().unwrap_or("user");
    let message = a.message.as_deref().unwrap_or("fs.edit");
    let sid = ws_session(&c, &ws.id)?;
    let recorded =
        live::snapshot(d, &mut c, std::slice::from_ref(&ws), &live::Meta { author, message, tool_call: a.tool_call.as_deref(), session: sid.as_deref() });
    journal_end(&c, &target.id)?;
    match recorded? {
        Some(cs) => {
            out.snap = snaps_of_changeset(&c, &cs)?.into_iter().next().map(|s| s.id);
            out.changeset = Some(cs);
        }
        None => out.status = FsEditStatus::Unchanged,
    }
    Ok(out)
}
