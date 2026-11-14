use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use rusqlite::params;
use tracefs_core::names;
use tracefs_core::path;
use tracefs_core::results::{BaseChanged, WsInfo, WsRegistered};

use super::live;
use crate::daemon::*;
use crate::store::{Undo, copy_dir_attrs, layers, reset_work};
use crate::sys;
use crate::tree::{Tree, llistxattr};

pub fn register(d: &Daemon, uid: u32, name: &str, from: &str) -> Result<WsRegistered> {
    if !names::valid_ws_name(name) {
        return Err(invalid(format!("invalid workspace name {name:?}")));
    }
    let src = fs::canonicalize(from).map_err(|e| invalid(format!("{from}: {e}")))?;
    d.check_ws_path(uid, &src)?;
    let meta = fs::metadata(&src)?;
    if !meta.is_dir() {
        return Err(invalid(format!("{} is not a directory", src.display())));
    }
    if uid != ROOT && meta.uid() != uid {
        return Err(invalid(format!("{} is owned by uid {}, not by the caller", src.display(), meta.uid())));
    }
    for own in [&d.paths.state, &d.paths.run] {
        let own = fs::canonicalize(own)?;
        if src.starts_with(&own) || own.starts_with(&src) {
            return Err(invalid("workspace overlaps the tracefs state or run directory"));
        }
    }
    let host_path = src.to_str().ok_or_else(|| invalid("workspace path must be UTF-8"))?.to_string();
    let _g = d.locks.lock([format!("wsname:{name}"), "live-register".to_string()]);
    let mut c = d.db()?;
    if ws_get(&c, name).is_ok() {
        return Err(api("exists", format!("workspace {name:?} already exists")));
    }
    for w in live_wss(&c)? {
        if src.starts_with(&w.host_path) || Path::new(&w.host_path).starts_with(&src) {
            return Err(invalid(format!("{host_path} overlaps live workspace {} at {}", w.name, w.host_path)));
        }
    }
    let (ws, base, main, snap, cs) = (new_id(), new_id(), new_id(), new_id(), new_id());
    let mut undo = Undo::new(d);
    let base_dir = d.paths.base(&base);
    sys::subvol_create(&base_dir)?;
    undo.subvol(base_dir.clone());
    let l1 = base_dir.join("l").join("1");
    fs::create_dir_all(&l1)?;
    sys::run(Command::new("cp").arg("-ax").arg("--reflink=auto").arg(src.join(".")).arg(&l1))?;
    copy_dir_attrs(&src, &l1)?;
    sys::set_readonly(&base_dir)?;
    let b = d.paths.branch(&main);
    sys::snapshot(&base_dir, &b, false)?;
    undo.subvol(b.clone());
    fs::create_dir(b.join("upper"))?;
    copy_dir_attrs(&src, &b.join("upper"))?;
    reset_work(&b)?;
    d.snap_branch(&main, &snap)?;
    undo.subvol(d.paths.snap(&snap));
    let aside = live::adopt(d, &src, &main, &mut undo)?;
    let t = c.transaction()?;
    let now = now_ms();
    t.execute(
        "INSERT INTO workspaces(id, name, host_path, uid, gid, main_branch, created_at, live) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![ws, name, host_path, meta.uid(), meta.gid(), main, now, true],
    )?;
    t.execute("INSERT INTO bases(id, ws, layers, created_at) VALUES (?1,?2,1,?3)", params![base, ws, now])?;
    t.execute("INSERT INTO branches(id, ws, base, kind, owner, head_snap) VALUES (?1,?2,?3,'main',?2,?4)", params![main, ws, base, snap])?;
    t.execute(
        "INSERT INTO changesets(id, owner_kind, owner, author, message, created_at) VALUES (?1,'workspace',?2,'tracefs','register',?3)",
        params![cs, ws, now],
    )?;
    t.execute("INSERT INTO snaps(id, branch, ws, changeset, base, created_at) VALUES (?1,?2,?3,?4,?5,?6)", params![snap, main, ws, cs, base, now])?;
    t.commit()?;
    undo.commit();
    let mut stale = Vec::new();
    d.watch.add(&ws, &d.paths.branch(&main).join("upper"));
    if let Some(a) = aside {
        stale = sys::users_of(&a);
        if let Err(e) = fs::remove_dir_all(&a) {
            eprintln!("tracefs-daemon: remove {}: {e}", a.display());
        }
    }
    Ok(WsRegistered {
        id: ws,
        name: name.into(),
        host_path,
        uid: meta.uid(),
        gid: meta.gid(),
        base,
        main_branch: main,
        head_snap: snap,
        changeset: cs,
        stale_processes: stale,
    })
}

pub fn list(d: &Daemon, uid: u32) -> Result<Vec<WsInfo>> {
    let c = d.db()?;
    let sql = "SELECT w.id, w.name, w.host_path, w.uid, w.gid, w.main_branch, b.head_snap, b.base, ba.layers, w.live
               FROM workspaces w JOIN branches b ON b.id = w.main_branch JOIN bases ba ON ba.id = b.base
               WHERE ?1 = 0 OR w.uid = ?1 ORDER BY w.name";
    let mut out: Vec<WsInfo> = rows(&c, sql, [uid], |r| {
        Ok(WsInfo {
            id: r.get(0)?,
            name: r.get(1)?,
            host_path: r.get(2)?,
            uid: r.get(3)?,
            gid: r.get(4)?,
            main_branch: r.get(5)?,
            head_snap: r.get(6)?,
            base: r.get(7)?,
            layers: r.get(8)?,
            live: r.get(9)?,
            session: None,
            pending: false,
        })
    })?;
    for w in &mut out {
        w.session = ws_session(&c, &w.id)?;
        w.pending = d.watch.is_dirty(&w.id);
    }
    Ok(out)
}

fn reseat_main(d: &Daemon, ws: &Ws, base: &str, layer_count: usize, from_snap: &str, message: &str) -> Result<BaseChanged> {
    if !ws.live {
        return reseat_subvol(d, ws, base, layer_count, from_snap, message);
    }
    live::suspend(d, ws)?;
    let r = reseat_subvol(d, ws, base, layer_count, from_snap, message);
    live::attach(d, ws)?;
    r
}

fn reseat_subvol(d: &Daemon, ws: &Ws, base: &str, layer_count: usize, from_snap: &str, message: &str) -> Result<BaseChanged> {
    let main = &ws.main_branch;
    let b = d.paths.branch(main);
    let old_upper = d.paths.snap(from_snap).join("upper");
    d.unmount_branch(main)?;
    let tmp = d.paths.state.join("branches").join(format!("{main}.new"));
    d.delete_subvol(&tmp)?;
    sys::snapshot(&d.paths.base(base), &tmp, false)?;
    fs::create_dir(tmp.join("upper"))?;
    copy_dir_attrs(&old_upper, &tmp.join("upper"))?;
    reset_work(&tmp)?;
    d.delete_subvol(&b)?;
    fs::rename(&tmp, &b)?;
    let snap = new_id();
    d.snap_branch(main, &snap)?;
    let cs = new_id();
    let mut c = d.db()?;
    c.execute(
        "INSERT INTO bases(id, ws, layers, from_snap, created_at) VALUES (?1,?2,?3,?4,?5)",
        params![base, ws.id, layer_count as i64, from_snap, now_ms()],
    )?;
    record_changeset(
        &mut c,
        &NewChangeset::new(cs.clone(), "workspace", &ws.id, "tracefs", message),
        &[NewSnap {
            id: snap.clone(),
            branch: main.clone(),
            ws: ws.id.clone(),
            parent: Some(from_snap.to_string()),
            merge_parent: None,
            base: base.to_string(),
        }],
    )?;
    Ok(BaseChanged { ws: ws.name.clone(), base: base.into(), layers: layer_count as u32, head_snap: snap, changeset: cs })
}

fn main_head(d: &Daemon, ws_key: &str) -> Result<(Ws, Branch)> {
    let mut c = d.db()?;
    let ws = ws_get(&c, ws_key)?;
    if ws.live {
        live::sync(d, &mut c, &ws, "before base change")?;
    }
    let main = branch_get(&c, &ws.main_branch)?;
    Ok((ws, main))
}

pub fn promote(d: &Daemon, uid: u32, snap: &str) -> Result<BaseChanged> {
    let c = d.db()?;
    let s = snap_get(&c, snap)?;
    let ws = ws_for(&c, uid, &s.ws).map_err(|_| not_found("snapshot", snap))?;
    drop(c);
    let _g = d.locks.lock(ws.keys());
    let (ws, main) = main_head(d, &ws.id)?;
    if main.head_snap != s.id {
        return Err(bad_state("only the current head of the main branch can be promoted"));
    }
    let n = layers(&d.paths.snap(&s.id))?.len();
    if n + 1 > d.cfg.max_layers {
        return flatten_locked(d, &ws, &main);
    }
    let base = new_id();
    let mut undo = Undo::new(d);
    let (dir, n) = base_from(d, &s.id, &base, &mut undo)?;
    sys::set_readonly(&dir)?;
    undo.commit();
    reseat_main(d, &ws, &base, n, &s.id, "promote base")
}

fn base_from<'a>(d: &'a Daemon, snap: &str, base: &str, undo: &mut Undo<'a>) -> Result<(PathBuf, usize)> {
    let dir = d.paths.base(base);
    sys::snapshot(&d.paths.snap(snap), &dir, false)?;
    undo.subvol(dir.clone());
    let n = layers(&dir)?.len() + 1;
    fs::rename(dir.join("upper"), dir.join("l").join(n.to_string()))?;
    fs::remove_dir_all(dir.join("work"))?;
    Ok((dir, n))
}

pub fn flatten(d: &Daemon, uid: u32, ws: &str) -> Result<BaseChanged> {
    let c = d.db()?;
    let ws = ws_for(&c, uid, ws)?;
    drop(c);
    let _g = d.locks.lock(ws.keys());
    let (ws, main) = main_head(d, &ws.id)?;
    flatten_locked(d, &ws, &main)
}

fn flatten_locked(d: &Daemon, ws: &Ws, main: &Branch) -> Result<BaseChanged> {
    let snap = main.head_snap.clone();
    let base = new_id();
    let mut undo = Undo::new(d);
    let (dir, _) = base_from(d, &snap, &base, &mut undo)?;
    let (fu, fw) = (dir.join("flat-upper"), dir.join("flat-work"));
    fs::create_dir(&fu)?;
    fs::create_dir(&fw)?;
    let ls = layers(&dir)?;
    copy_dir_attrs(&ls[0], &fu)?;
    let fd = sys::overlay(&ls, Some((fu.clone(), fw.clone())))?;
    let mnt = d.paths.views().join(format!("flatten-{base}"));
    fs::create_dir_all(&mnt)?;
    sys::move_mount(&fd, &mnt)?;
    let walked = Tree::open(&mnt).map_err(Into::into).and_then(|t| copy_up_all(&t, b""));
    sys::unmount_and_remove(&mnt)?;
    walked?;
    strip_overlay_metadata(&fu)?;
    for l in fs::read_dir(dir.join("l"))? {
        fs::remove_dir_all(l?.path())?;
    }
    fs::rename(&fu, dir.join("l").join("1"))?;
    fs::remove_dir_all(&fw)?;
    sys::set_readonly(&dir)?;
    undo.commit();
    reseat_main(d, ws, &base, 1, &snap, "flatten base")
}

fn copy_up_all(t: &Tree, rel: &[u8]) -> Result<()> {
    let st = t.lstat(rel)?.ok_or_else(|| anyhow!("entry vanished during flatten"))?;
    if st.is_dir() {
        for n in t.list(rel)? {
            copy_up_all(t, &path::join(rel, &n))?;
        }
    }
    t.set_times(rel, st.atime, st.mtime).with_context(|| format!("copy up {}", String::from_utf8_lossy(rel)))?;
    Ok(())
}

fn strip_overlay_metadata(root: &Path) -> Result<()> {
    let t = Tree::open(root)?;
    fn walk(root: &Path, t: &Tree, rel: &[u8]) -> Result<()> {
        let Some(st) = t.lstat(rel)? else { return Ok(()) };
        if st.is_whiteout() {
            t.unlink(rel, false)?;
            return Ok(());
        }
        let full = root.join(std::ffi::OsStr::from_bytes(rel));
        for k in llistxattr(&full)?.into_iter().filter(|k| k.starts_with(b"trusted.overlay.")) {
            rustix::fs::lremovexattr(&full, &k)?;
        }
        if st.is_dir() {
            for n in t.list(rel)? {
                walk(root, t, &path::join(rel, &n))?;
            }
        }
        Ok(())
    }
    walk(root, &t, b"")
}
