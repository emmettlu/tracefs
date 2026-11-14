use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::Connection;
use rustix::io::Errno;
use tracefs_core::merge::{self, Action, Conflict, Entry, FType, Inspect, Meta, Plan, Resolution, Side, Step};
use tracefs_core::path::{self, PathName, RelPath};
use tracefs_core::proto::{ResolutionAction, ResolutionSpec};
use tracefs_core::rules::{self, Rules};

use crate::daemon::{Daemon, invalid};
use crate::diff;
use crate::store::SnapTrees;
use crate::tree::{Stat, Tree, set_user_xattrs_fd, timestamps};

struct Views {
    o: SnapTrees,
    s: SnapTrees,
    t: SnapTrees,
    entries: HashMap<(Side, RelPath), Option<Entry>>,
    cache: HashMap<(Side, RelPath), Vec<u8>>,
}

impl Views {
    fn snap(&self, side: Side) -> &SnapTrees {
        match side {
            Side::Base => &self.o,
            Side::Source => &self.s,
            Side::Target => &self.t,
        }
    }

    fn tree(&self, side: Side) -> &Tree {
        &self.snap(side).view
    }
}

impl Inspect for Views {
    fn entry(&mut self, side: Side, p: &[u8]) -> Option<Entry> {
        let key = (side, p.to_vec());
        if let Some(e) = self.entries.get(&key) {
            return *e;
        }
        let e = self.tree(side).lstat(p).ok().flatten().map(|st| Entry {
            ftype: st.ftype(),
            meta: st.meta(),
            size: if st.ftype() == FType::Reg { st.size } else { 0 },
        });
        self.entries.insert(key, e);
        e
    }
    fn content(&mut self, side: Side, p: &[u8]) -> Vec<u8> {
        let key = (side, p.to_vec());
        if let Some(v) = self.cache.get(&key) {
            return v.clone();
        }
        let t = self.tree(side);
        let v = match t.lstat(p).ok().flatten().map(|s| s.ftype()) {
            Some(FType::Symlink) => t.readlink(p).unwrap_or_default(),
            Some(FType::Reg) => t.open_read(p).and_then(diff::sha256).unwrap_or_default(),
            _ => Vec::new(),
        };
        self.cache.insert(key, v.clone());
        v
    }
    fn is_text(&mut self, side: Side, p: &[u8]) -> bool {
        let Ok(f) = self.tree(side).open_read(p) else { return false };
        let mut buf = Vec::with_capacity(8000);
        f.take(8000).read_to_end(&mut buf).is_ok() && !buf.contains(&0)
    }
    fn xattrs(&mut self, side: Side, p: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.tree(side).user_xattrs(p).unwrap_or_default()
    }
    fn ident(&mut self, side: Side, p: &[u8]) -> Option<u64> {
        diff::ident(&self.snap(side).side(), p)
    }
    fn mtime(&mut self, side: Side, p: &[u8]) -> (i64, i64) {
        let t = self.tree(side);
        let mut p = p;
        loop {
            if let Ok(Some(st)) = t.lstat(p) {
                return st.mtime;
            }
            if p.is_empty() {
                return (0, 0);
            }
            p = path::parent(p);
        }
    }
}

pub fn rules_of(view: &Tree) -> Rules {
    let read = |name: &str| -> Option<Vec<u8>> {
        let st = view.lstat(name.as_bytes()).ok()??;
        (st.ftype() == FType::Reg && st.size <= 1 << 20).then(|| view.read_all(name.as_bytes()).ok()).flatten()
    };
    let (r, warnings) = Rules::parse(read(rules::IGNORE_FILE).as_deref(), read(rules::MERGE_FILE).as_deref());
    for w in warnings {
        eprintln!("tracefs-daemon: {w}");
    }
    r
}

pub enum Outcome {
    UpToDate,
    Conflicts(Vec<Conflict>, Vec<RelPath>),
    Applied { actions: usize, unused: Vec<RelPath> },
}

pub fn path_names(ps: &[RelPath]) -> Vec<PathName> {
    ps.iter().map(|p| PathName::new(p)).collect()
}

pub fn merge(d: &Daemon, c: &Connection, o: &str, s: &str, t: &str, t_live: &std::path::Path, resolutions: &BTreeMap<RelPath, Resolution>) -> Result<Outcome> {
    if o == s {
        return Ok(Outcome::UpToDate);
    }
    let mut views =
        Views { o: SnapTrees::open(d, c, o)?, s: SnapTrees::open(d, c, s)?, t: SnapTrees::open(d, c, t)?, entries: HashMap::new(), cache: HashMap::new() };
    let src = diff::diff(Some(&views.o.side()), &views.s.side(), views.o.base == views.s.base, false)?;
    if src.kinds.is_empty() {
        return Ok(Outcome::UpToDate);
    }
    let dst = diff::diff(Some(&views.o.side()), &views.t.side(), views.o.base == views.t.base, false)?;
    let rules = rules_of(&views.t.view);
    let idents = (crate::ops::lineage(c, o, s)?, crate::ops::lineage(c, o, t)?);
    let input = merge::Input { source: &src.kinds, target: &dst.kinds, idents, resolutions, rules: &rules };
    let mut plan = merge::plan(&input, &mut views);
    let mut merged: HashMap<RelPath, Vec<u8>> = HashMap::new();
    if plan.conflicts.is_empty() {
        for (r, st) in &plan.writes {
            if let Action::TextMerge { union, .. } = st.action {
                match text_merge(d, &views, st, union)? {
                    Some(m) => {
                        merged.insert(r.clone(), m);
                    }
                    None => plan.conflicts.push(Conflict { path: r.clone(), reason: "text" }),
                }
            }
        }
    }
    if !plan.conflicts.is_empty() {
        return Ok(Outcome::Conflicts(plan.conflicts, plan.unused_resolutions));
    }
    crate::failpoint("merge-before-apply");
    let live = Tree::open(t_live)?;
    apply(&plan, &views, &live, &merged)?;
    Ok(Outcome::Applied { actions: plan.len(), unused: plan.unused_resolutions })
}

fn text_merge(d: &Daemon, v: &Views, st: &Step, union: bool) -> Result<Option<Vec<u8>>> {
    let dir = d.paths.tmp().join(format!("merge-{}", crate::tree::nanos()));
    fs::create_dir_all(&dir)?;
    let res = (|| -> Result<Option<Vec<u8>>> {
        for (name, tree, p) in [("o", &v.o.view, &st.o), ("s", &v.s.view, &st.s), ("t", &v.t.view, &st.t)] {
            let p = p.as_ref().ok_or_else(|| anyhow!("text merge without a {name} path"))?;
            fs::write(dir.join(name), tree.read_all(p)?)?;
        }
        let mut cmd = Command::new("git");
        cmd.args(["merge-file", "-p", "-q"]);
        if union {
            cmd.arg("--union");
        }
        let out = cmd.arg(dir.join("t")).arg(dir.join("o")).arg(dir.join("s")).output().context("run git merge-file")?;
        match out.status.code() {
            Some(0) => Ok(Some(out.stdout)),
            Some(n) if n > 0 => Ok(None),
            _ => Err(anyhow!("git merge-file failed: {}", String::from_utf8_lossy(&out.stderr))),
        }
    })();
    let _ = fs::remove_dir_all(&dir);
    res
}

pub fn remove(t: &Tree, p: &[u8]) -> Result<()> {
    let Some(st) = t.lstat(p)? else { return Ok(()) };
    if st.is_dir() {
        for n in t.list(p)? {
            remove(t, &path::join(p, &n))?;
        }
    }
    t.unlink(p, st.is_dir()).with_context(|| format!("remove {}", String::from_utf8_lossy(p)))?;
    Ok(())
}

fn move_entry(t: &Tree, from: &[u8], to: &[u8]) -> Result<()> {
    match t.rename_noreplace(from, to) {
        Ok(()) => Ok(()),
        Err(e) if Errno::from_io_error(&e) == Some(Errno::XDEV) => {
            copy_within(t, from, to)?;
            remove(t, from)
        }
        Err(e) => Err(e).with_context(|| format!("move {} to {}", String::from_utf8_lossy(from), String::from_utf8_lossy(to))),
    }
}

fn copy_within(t: &Tree, from: &[u8], to: &[u8]) -> Result<()> {
    let st = t.lstat(from)?.ok_or_else(|| anyhow!("entry vanished during a move"))?;
    let xattrs = t.user_xattrs(from)?;
    match st.ftype() {
        FType::Dir => {
            t.mkdir(to, 0o700)?;
            for n in t.list(from)? {
                copy_within(t, &path::join(from, &n), &path::join(to, &n))?;
            }
            apply_meta(t, to, st.meta(), FType::Dir, &xattrs)?;
            t.set_times(to, st.atime, st.mtime)?;
        }
        FType::Reg => write_file(t, to, &mut t.open_read(from)?, st.meta(), Some((st.atime, st.mtime)), &xattrs, false)?,
        FType::Symlink | FType::Fifo => special(t, to, t, from, &st, &xattrs)?,
        FType::Other => bail!("unsupported file type at {}", String::from_utf8_lossy(from)),
    }
    Ok(())
}

pub fn write_file(
    t: &Tree,
    p: &[u8],
    src: &mut impl Read,
    meta: Meta,
    mtime: Option<((i64, i64), (i64, i64))>,
    xattrs: &[(Vec<u8>, Vec<u8>)],
    in_place: bool,
) -> Result<()> {
    let mut fill = |f: &mut fs::File| -> Result<()> {
        std::io::copy(src, f)?;
        f.flush()?;
        std::os::unix::fs::fchown(&*f, Some(meta.uid), Some(meta.gid))?;
        f.set_permissions(fs::Permissions::from_mode(meta.mode))?;
        set_user_xattrs_fd(&*f, xattrs)?;
        if let Some((at, mt)) = mtime {
            rustix::fs::futimens(&*f, &timestamps(at, mt))?;
        }
        Ok(())
    };
    if in_place
        && t.lstat(p)?.is_some_and(|s| s.ftype() == FType::Reg)
        && let Ok(fd) = t.open_entry(p, rustix::fs::OFlags::WRONLY | rustix::fs::OFlags::TRUNC | rustix::fs::OFlags::NONBLOCK)
    {
        return fill(&mut fs::File::from(fd));
    }
    let (mut f, tmp) = t.create_temp(p)?;
    let res = (|| -> Result<()> {
        fill(&mut f)?;
        if t.lstat(p)?.is_some_and(|s| s.is_dir()) {
            remove(t, p)?;
        }
        t.rename_into(p, &tmp)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = t.unlink(&path::join(path::parent(p), &tmp), false);
    }
    res
}

fn special(t: &Tree, to: &[u8], src: &Tree, from: &[u8], st: &Stat, xattrs: &[(Vec<u8>, Vec<u8>)]) -> Result<()> {
    if st.ftype() == FType::Symlink {
        t.symlink(to, &src.readlink(from)?)?;
    } else {
        t.mkfifo(to, 0o600)?;
    }
    apply_meta(t, to, st.meta(), st.ftype(), xattrs)?;
    Ok(t.set_times(to, st.atime, st.mtime)?)
}

fn apply_meta(t: &Tree, p: &[u8], meta: Meta, ftype: FType, xattrs: &[(Vec<u8>, Vec<u8>)]) -> Result<()> {
    t.lchown(p, meta.uid, meta.gid)?;
    if ftype != FType::Symlink {
        t.chmod(p, meta.mode)?;
        t.set_user_xattrs(p, xattrs)?;
    }
    Ok(())
}

fn copy_entry(v: &Views, live: &Tree, s: &[u8], r: &[u8], in_place: bool) -> Result<()> {
    let st = v.s.view.lstat(s)?.ok_or_else(|| anyhow!("source entry vanished"))?;
    let cur = live.lstat(r)?;
    let xattrs = v.s.view.user_xattrs(s)?;
    match st.ftype() {
        FType::Dir => {
            match cur {
                Some(c) if c.is_dir() => {}
                Some(_) => {
                    live.unlink(r, false)?;
                    live.mkdir(r, 0o700)?;
                }
                None => live.mkdir(r, 0o700)?,
            }
            apply_meta(live, r, st.meta(), FType::Dir, &xattrs)?;
        }
        FType::Reg => {
            let mut f = v.s.view.open_read(s)?;
            write_file(live, r, &mut f, st.meta(), Some((st.atime, st.mtime)), &xattrs, in_place)?;
        }
        FType::Symlink | FType::Fifo => {
            if cur.is_some() {
                remove(live, r)?;
            }
            special(live, r, &v.s.view, s, &st, &xattrs)?;
        }
        FType::Other => bail!("unsupported file type at {}", String::from_utf8_lossy(s)),
    }
    Ok(())
}

fn need<'a>(p: &'a Option<RelPath>, what: &str) -> Result<&'a [u8]> {
    p.as_deref().ok_or_else(|| anyhow!("merge step without a {what} path"))
}

fn write_step(v: &Views, live: &Tree, r: &[u8], st: &Step, merged: &HashMap<RelPath, Vec<u8>>) -> Result<()> {
    let same = st.t.is_some();
    match &st.action {
        Action::Write => copy_entry(v, live, need(&st.s, "source")?, r, same),
        Action::WriteContent => {
            let (s, t) = (need(&st.s, "source")?, need(&st.t, "target")?);
            let ss = v.s.view.lstat(s)?.ok_or_else(|| anyhow!("source entry vanished"))?;
            let ts = v.t.view.lstat(t)?.ok_or_else(|| anyhow!("target entry vanished"))?;
            if ss.ftype() == FType::Symlink {
                remove(live, r)?;
                live.symlink(r, &v.s.view.readlink(s)?)?;
                return apply_meta(live, r, ts.meta(), FType::Symlink, &[]);
            }
            write_file(live, r, &mut v.s.view.open_read(s)?, ts.meta(), Some((ss.atime, ss.mtime)), &v.t.view.user_xattrs(t)?, same)
        }
        Action::Meta => {
            let s = need(&st.s, "source")?;
            let ss = v.s.view.lstat(s)?.ok_or_else(|| anyhow!("source entry vanished"))?;
            apply_meta(live, r, ss.meta(), ss.ftype(), &v.s.view.user_xattrs(s)?)
        }
        Action::TextMerge { meta_from, .. } => {
            let (tree, p) = if *meta_from == Side::Source { (&v.s.view, need(&st.s, "source")?) } else { (&v.t.view, need(&st.t, "target")?) };
            let ms = tree.lstat(p)?.ok_or_else(|| anyhow!("entry vanished"))?;
            write_file(live, r, &mut &merged[r][..], ms.meta(), None, &tree.user_xattrs(p)?, same)
        }
        Action::Content(data) => {
            let reg = |tree: &Tree, p: &Option<RelPath>| p.as_deref().and_then(|p| tree.lstat(p).ok().flatten()).filter(|s| s.ftype() == FType::Reg);
            let meta = match reg(&v.t.view, &st.t).or_else(|| reg(&v.s.view, &st.s)) {
                Some(s) => s.meta(),
                None => {
                    let parent = live.lstat(path::parent(r))?.ok_or_else(|| anyhow!("parent missing"))?;
                    Meta { mode: 0o644, uid: parent.uid, gid: parent.gid }
                }
            };
            if live.lstat(r)?.is_some_and(|s| !s.is_dir() && s.ftype() != FType::Reg) {
                remove(live, r)?;
            }
            write_file(live, r, &mut &data[..], meta, None, &[], same)
        }
    }
}

fn apply(plan: &Plan, v: &Views, live: &Tree, merged: &HashMap<RelPath, Vec<u8>>) -> Result<()> {
    let depth = |p: &[u8]| p.iter().filter(|&&c| c == b'/').count();
    let mut order: Vec<&merge::Move> = plan.moves.iter().collect();
    order.sort_by_key(|m| std::cmp::Reverse(depth(&m.from)));
    let mut aside: Vec<(RelPath, RelPath)> = Vec::new();
    let mut place: BTreeMap<RelPath, RelPath> = BTreeMap::new();
    for (k, m) in order.into_iter().enumerate() {
        let tmp = format!(".tracefs-move-{}-{}-{k}", std::process::id(), crate::tree::nanos()).into_bytes();
        move_entry(live, &m.from, &tmp)?;
        aside.push((m.from.clone(), tmp.clone()));
        place.insert(m.to.clone(), tmp);
    }
    let now = |p: &[u8]| -> RelPath {
        match aside.iter().filter(|(f, _)| p == f.as_slice() || path::is_under(p, f)).max_by_key(|(f, _)| f.len()) {
            Some((f, tmp)) => {
                let mut r = tmp.clone();
                r.extend_from_slice(&p[f.len()..]);
                r
            }
            None => p.to_vec(),
        }
    };
    for p in plan.deletes.iter().rev() {
        remove(live, &now(p))?;
    }
    let keys: BTreeSet<&RelPath> = plan.writes.keys().chain(place.keys()).collect();
    for r in keys {
        if let Some(tmp) = place.get(r) {
            move_entry(live, tmp, r)?;
        }
        if let Some(st) = plan.writes.get(r) {
            write_step(v, live, r, st, merged)?;
        }
    }
    Ok(())
}

pub fn resolutions_for(specs: &[ResolutionSpec], ws_id: &str, ws_name: &str) -> Result<BTreeMap<RelPath, Resolution>> {
    let mut out = BTreeMap::new();
    for r in specs.iter().filter(|r| r.ws == ws_id || r.ws == ws_name) {
        let p = r.path.to_bytes().ok_or_else(|| invalid("bad resolution path"))?;
        if !path::is_valid(&p) {
            return Err(invalid(format!("invalid resolution path {:?}", String::from_utf8_lossy(&p))));
        }
        let res = match r.action {
            ResolutionAction::Ours => Resolution::Ours,
            ResolutionAction::Theirs => Resolution::Theirs,
            ResolutionAction::Content => {
                Resolution::Content(r.content.as_deref().and_then(path::b64_decode).ok_or_else(|| invalid("content resolution needs content"))?)
            }
        };
        out.insert(p, res);
    }
    Ok(out)
}
