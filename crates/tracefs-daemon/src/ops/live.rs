use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use tracefs_core::merge::Resolution;
use tracefs_core::path::RelPath;
use tracefs_core::proto::{Changeset, Reset, Revert};
use tracefs_core::results::{Applied, LiveChangeset, LiveSpan, MergeResult, MergeStatus, WsDetached};

use super::history::resolve;
use super::{merge_into, snap_branches};
use crate::daemon::*;
use crate::diff;
use crate::merging;
use crate::store::{SnapTrees, Undo, copy_dir_attrs};
use crate::sys;

pub fn attach(d: &Daemon, ws: &Ws) -> Result<()> {
    d.mount_branch_at(&ws.main_branch, Path::new(&ws.host_path))?;
    d.watch.add(&ws.id, &d.paths.branch(&ws.main_branch).join("upper"));
    Ok(())
}

fn sibling(host: &Path, what: &str) -> PathBuf {
    let name = host.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    host.with_file_name(format!(".{name}.tracefs-{what}-{}", new_id()))
}

pub fn adopt<'a>(d: &'a Daemon, host: &Path, main: &str, undo: &mut Undo<'a>) -> Result<Option<PathBuf>> {
    let aside = sibling(host, "orig");
    fs::create_dir(&aside).with_context(|| format!("create {}", aside.display()))?;
    copy_dir_attrs(host, &aside)?;
    let moved = match sys::exchange(host, &aside) {
        Ok(()) => {
            let (h, a) = (host.to_path_buf(), aside.clone());
            undo.defer(move || {
                let _ = sys::exchange(&h, &a);
                let _ = fs::remove_dir(&a);
            });
            Some(aside)
        }
        Err(e) => {
            eprintln!("tracefs-daemon: cannot move {} aside ({e}); its old content stays under the mount", host.display());
            let _ = fs::remove_dir(&aside);
            None
        }
    };
    d.mount_branch_at(main, host)?;
    let h = host.to_path_buf();
    undo.defer(move || {
        let _ = sys::unmount(&h);
    });
    Ok(moved)
}

pub struct Meta<'a> {
    pub author: &'a str,
    pub message: &'a str,
    pub tool_call: Option<&'a str>,
    pub session: Option<&'a str>,
}

pub fn changed(d: &Daemon, c: &Connection, head: &str, new: &str, base: &str) -> Result<bool> {
    let a = SnapTrees::open(d, c, head)?;
    let b = SnapTrees::at(d, new, base.to_string())?;
    Ok(!diff::filtered(&a, &b)?.kinds.is_empty())
}

pub fn snapshot(d: &Daemon, c: &mut Connection, wss: &[Ws], m: &Meta) -> Result<Option<String>> {
    let mut undo = Undo::new(d);
    let taken = snap_branches(d, c, wss.iter().map(|w| w.main_branch.as_str()), &mut undo)?;
    let mut any = false;
    for (b, id) in &taken {
        if changed(d, c, &b.head_snap, id, &b.base)? {
            any = true;
            break;
        }
    }
    if !any {
        return Ok(None);
    }
    let cs = new_id();
    let owner = &wss[0].id;
    let snaps: Vec<NewSnap> = taken.into_iter().map(|(b, id)| NewSnap::after(&b, id, None)).collect();
    record_changeset(
        c,
        &NewChangeset { tool_call: m.tool_call, session: m.session, ..NewChangeset::new(cs.clone(), "workspace", owner, m.author, m.message) },
        &snaps,
    )?;
    undo.commit();
    Ok(Some(cs))
}

pub fn sync(d: &Daemon, c: &mut Connection, ws: &Ws, message: &str) -> Result<Option<String>> {
    let session = ws_session(c, &ws.id)?;
    snapshot(d, c, std::slice::from_ref(ws), &Meta { author: "tracefs", message, tool_call: None, session: session.as_deref() })
}

fn reload_live(c: &Connection, wss: &[Ws]) -> Result<Vec<Ws>> {
    wss.iter()
        .map(|w| {
            let w = ws_get(c, &w.id)?;
            if !w.live {
                return Err(invalid(format!("{} is detached", w.name)));
            }
            Ok(w)
        })
        .collect()
}

pub fn auto_snapshot(d: &Daemon, ws_id: &str) -> Result<()> {
    let ws = ws_get(&d.db()?, ws_id)?;
    let _g = d.locks.lock(ws.keys());
    let mut c = d.db()?;
    let ws = ws_get(&c, ws_id)?;
    if !ws.live {
        return Ok(());
    }
    let session = ws_session(&c, &ws.id)?;
    snapshot(d, &mut c, &[ws], &Meta { author: "auto", message: "auto", tool_call: None, session: session.as_deref() })?;
    Ok(())
}

pub fn session_wss(c: &Connection, session: &str) -> Result<Vec<Ws>> {
    let ids: Vec<String> = col(c, "SELECT ws FROM session_live WHERE session = ?1 AND end_snap IS NULL ORDER BY ws", [session])?;
    ids.iter().map(|w| ws_get(c, w)).collect()
}

pub fn changeset(d: &Daemon, uid: u32, a: Changeset) -> Result<LiveChangeset> {
    let c0 = d.db()?;
    let session = a.session.as_deref().map(|s| session_for(&c0, uid, s)).transpose()?;
    let wss = match (&session, a.ws.is_empty()) {
        (_, false) => a.ws.iter().map(|w| ws_for(&c0, uid, w)).collect::<Result<Vec<_>>>()?,
        (Some(s), true) => session_wss(&c0, &s.id)?,
        (None, true) => return Err(invalid("changeset needs workspaces or --session")),
    };
    if wss.is_empty() {
        return Err(invalid("the session labels no live workspace"));
    }
    drop(c0);
    let _g = d.locks.lock(wss.iter().flat_map(Ws::keys));
    let mut c = d.db()?;
    let wss = reload_live(&c, &wss)?;
    let sid = match &session {
        Some(s) => {
            session_get(&c, &s.id)?.ensure_active()?;
            Some(s.id.clone())
        }
        None => ws_session(&c, &wss[0].id)?,
    };
    let m = Meta {
        author: a.author.as_deref().unwrap_or("user"),
        message: a.message.as_deref().unwrap_or(""),
        tool_call: a.tool_call.as_deref(),
        session: sid.as_deref(),
    };
    let cs = snapshot(d, &mut c, &wss, &m)?;
    Ok(LiveChangeset { unchanged: cs.is_none(), changeset: cs, session: sid })
}

enum Target {
    Main(Ws),
    Proc(super::procs::Proc, Ws, String),
}

impl Target {
    fn of(c: &Connection, s: &Snap) -> Result<Target> {
        let ws = ws_get(c, &s.ws)?;
        if s.branch != ws.main_branch
            && let Some(p) = super::procs::running_proc_of_branch(c, &s.branch)?
        {
            return Ok(Target::Proc(p, ws, s.branch.clone()));
        }
        Ok(Target::Main(ws))
    }

    fn ws(&self) -> &Ws {
        match self {
            Target::Main(w) | Target::Proc(_, w, _) => w,
        }
    }

    fn keys(&self) -> Vec<String> {
        let w = self.ws();
        let mut k = w.keys().to_vec();
        if let Target::Proc(p, _, b) = self {
            k.extend([k_proc(&p.id), k_branch(b)]);
        }
        k
    }
}

fn apply(d: &Daemon, c: &mut Connection, t: &Target, o: Option<String>, s: &str, res: &BTreeMap<RelPath, Resolution>, message: &str) -> Result<MergeResult> {
    let (mut out, _) = match t {
        Target::Main(ws) => {
            sync(d, c, ws, &format!("before {message}"))?;
            let main = branch_get(c, &ws.main_branch)?;
            let o = o.unwrap_or_else(|| main.head_snap.clone());
            merge_into(d, c, s, &main, Some(&o), res, ("workspace", &ws.id), message)?
        }
        Target::Proc(p, _, branch) => {
            let frozen = super::procs::frozen_for_write(d, c, p, branch, &format!("before {message}"))?;
            let b = branch_get(c, branch)?;
            let o = o.unwrap_or_else(|| b.head_snap.clone());
            let r = merge_into(d, c, s, &b, Some(&o), res, ("process", &p.id), message)?;
            drop(frozen);
            r
        }
    };
    out.ws = t.ws().name.clone();
    Ok(out)
}

fn applied(results: Vec<MergeResult>) -> Applied {
    let conflict = results.iter().any(|r| r.status == MergeStatus::Conflict);
    Applied { results, conflict }
}

fn lock_targets<'a, 'b>(d: &'a Daemon, targets: impl Iterator<Item = &'b Target> + Clone) -> Result<(Guard<'a>, Connection)> {
    let g = d.locks.lock(targets.clone().flat_map(Target::keys));
    let c = d.db()?;
    reload_live(&c, &targets.map(|t| t.ws().clone()).collect::<Vec<_>>())?;
    Ok((g, c))
}

pub fn reset(d: &Daemon, uid: u32, a: Reset) -> Result<Applied> {
    let c0 = d.db()?;
    let snaps = resolve(&c0, &a.target)?;
    check_snaps(&c0, uid, &a.target, &snaps)?;
    let targets = snaps.iter().map(|s| Target::of(&c0, s)).collect::<Result<Vec<_>>>()?;
    drop(c0);
    let (_g, mut c) = lock_targets(d, targets.iter())?;
    let mut results = Vec::new();
    for (s, t) in snaps.iter().zip(&targets) {
        if !s.present {
            return Err(bad_state(format!("snapshot {} no longer exists", s.id)));
        }
        let message = format!("reset to {}", a.target);
        let mut out = match t {
            Target::Proc(p, _, branch) => super::procs::reset_own(d, &mut c, p, branch, &s.id, &message)?,
            Target::Main(_) => apply(d, &mut c, t, None, &s.id, &BTreeMap::new(), &message)?,
        };
        out.ws = t.ws().name.clone();
        results.push(out);
    }
    Ok(applied(results))
}

pub fn revert(d: &Daemon, uid: u32, a: Revert) -> Result<Applied> {
    let c0 = d.db()?;
    let mut plan: Vec<(Target, Option<String>, String)> = Vec::new();
    match (&a.changeset, &a.session, &a.from, &a.to) {
        (Some(cs), None, None, None) => {
            let snaps = snaps_of_changeset(&c0, cs)?;
            check_snaps(&c0, uid, cs, &snaps)?;
            for s in snaps {
                let parent = s.parent.clone().ok_or_else(|| invalid(format!("snapshot {} has no parent", s.id)))?;
                plan.push((Target::of(&c0, &s)?, Some(s.id), parent));
            }
        }
        (None, Some(sess), None, None) => {
            let sess = session_for(&c0, uid, sess)?;
            let sql = "SELECT ws, start_snap, end_snap FROM session_live WHERE session = ?1 ORDER BY ws";
            let spans: Vec<(String, String, Option<String>)> = rows(&c0, sql, [&sess.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            for (w, start, end) in spans {
                plan.push((Target::Main(ws_for(&c0, uid, &w)?), end, start));
            }
        }
        (None, None, Some(from), Some(to)) => {
            let (fs, ts) = (resolve(&c0, from)?, resolve(&c0, to)?);
            check_snaps(&c0, uid, from, &fs)?;
            check_snaps(&c0, uid, to, &ts)?;
            for t in ts {
                let f = fs.iter().find(|f| f.ws == t.ws).ok_or_else(|| invalid(format!("{from} has no snapshot of the workspace of {to}")))?;
                if let Some(s) = [f, &t].into_iter().find(|s| !s.present) {
                    return Err(bad_state(format!("snapshot {} no longer exists", s.id)));
                }
                plan.push((Target::of(&c0, &t)?, Some(t.id), f.id.clone()));
            }
        }
        _ => return Err(invalid("revert needs one of: a changeset, --session, or --from with --to")),
    }
    if plan.is_empty() {
        return Err(invalid("nothing to revert"));
    }
    drop(c0);
    let (_g, mut c) = lock_targets(d, plan.iter().map(|p| &p.0))?;
    let what = match (&a.changeset, &a.session, &a.from, &a.to) {
        (Some(cs), ..) => cs.clone(),
        (_, Some(s), ..) => format!("session {s}"),
        (.., Some(f), Some(t)) => format!("{f}..{t}"),
        _ => String::new(),
    };
    let mut results = Vec::new();
    for (t, o, s) in &plan {
        let ws = t.ws();
        let res = merging::resolutions_for(&a.resolutions, &ws.id, &ws.name)?;
        results.push(apply(d, &mut c, t, o.clone(), s, &res, &format!("revert {what}"))?);
    }
    Ok(applied(results))
}

pub fn label(d: &Daemon, c: &mut Connection, session: &str, wss: &[Ws]) -> Result<Vec<LiveSpan>> {
    let mut out = Vec::new();
    for ws in wss {
        if let Some(s) = ws_session(c, &ws.id)? {
            return Err(bad_state(format!("{} is already labelled by active session {s}", ws.name)));
        }
        sync(d, c, ws, &format!("before session {session}"))?;
        let head = branch_get(c, &ws.main_branch)?.head_snap;
        c.execute("INSERT INTO session_live(session, ws, start_snap) VALUES (?1,?2,?3)", params![session, ws.id, head])?;
        out.push(LiveSpan { ws: ws.name.clone(), start_snap: head, end_snap: None, discard: None });
    }
    Ok(out)
}

pub fn unlabel(d: &Daemon, c: &mut Connection, session: &str, discard: bool) -> Result<Vec<LiveSpan>> {
    let mut out = Vec::new();
    for ws in session_wss(c, session)? {
        sync(d, c, &ws, &format!("end session {session}"))?;
        let head = branch_get(c, &ws.main_branch)?.head_snap;
        c.execute("UPDATE session_live SET end_snap = ?1 WHERE session = ?2 AND ws = ?3", params![head, session, ws.id])?;
        let start: String = c.query_row("SELECT start_snap FROM session_live WHERE session = ?1 AND ws = ?2", params![session, ws.id], |r| r.get(0))?;
        let discard = if discard {
            Some(apply(d, c, &Target::Main(ws.clone()), Some(head.clone()), &start, &BTreeMap::new(), &format!("discard session {session}"))?)
        } else {
            None
        };
        out.push(LiveSpan { ws: ws.name.clone(), start_snap: start, end_snap: Some(head), discard });
    }
    Ok(out)
}

pub fn detach(d: &Daemon, uid: u32, key: &str) -> Result<WsDetached> {
    let ws = ws_for(&d.db()?, uid, key)?;
    let _g = d.locks.lock(ws.keys());
    let mut c = d.db()?;
    let ws = reload_live(&c, &[ws])?.remove(0);
    if let Some(s) = ws_session(&c, &ws.id)? {
        return Err(bad_state(format!("{} is labelled by active session {s}; end it first", ws.name)));
    }
    let procs: Vec<String> = col(&c, "SELECT proc FROM proc_branches WHERE ws = ?1 AND state IN ('open', 'conflicted')", [&ws.id])?;
    if !procs.is_empty() {
        return Err(bad_state(format!("processes still have work on {}: {}", ws.name, procs.join(", "))));
    }
    sync(d, &mut c, &ws, "detach")?;
    let head = branch_get(&c, &ws.main_branch)?.head_snap;
    let host = PathBuf::from(&ws.host_path);
    let staging = sibling(&host, "detach");
    let copied = (|| -> Result<()> {
        let view = d.view(&head)?;
        fs::create_dir(&staging)?;
        sys::run(Command::new("cp").arg("-a").arg(view.path.join(".")).arg(&staging))?;
        copy_dir_attrs(&view.path, &staging)
    })();
    if let Err(e) = copied {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    if let Err(e) = sys::umount(&host, false) {
        let _ = fs::remove_dir_all(&staging);
        return Err(bad_state(format!("{} is in use ({e}): {}", host.display(), sys::users_of(&host).join(", "))));
    }
    d.watch.remove(&ws.id);
    sys::exchange(&host, &staging).with_context(|| format!("swap {} into place", staging.display()))?;
    c.execute("UPDATE workspaces SET live = 0 WHERE id = ?1", [&ws.id])?;
    if let Err(e) = fs::remove_dir_all(&staging) {
        eprintln!("tracefs-daemon: remove {}: {e}", staging.display());
    }
    Ok(WsDetached { ws: ws.name, head_snap: head })
}

pub fn suspend(d: &Daemon, ws: &Ws) -> Result<()> {
    let host = Path::new(&ws.host_path);
    sys::umount(host, false).map_err(|e| bad_state(format!("{} is in use ({e}): {}", host.display(), sys::users_of(host).join(", "))))?;
    d.watch.remove(&ws.id);
    Ok(())
}

pub fn remount_all(d: &Daemon) -> Result<()> {
    let c = d.db()?;
    for ws in live_wss(&c)? {
        let host = Path::new(&ws.host_path);
        if !sys::is_mountpoint(host) {
            fs::create_dir_all(host)?;
            if fs::read_dir(host)?.next().is_some() {
                eprintln!("tracefs-daemon: {} is not empty under the live mount; files written there while tracefs was down are hidden", host.display());
            }
            if let Err(e) = d.mount_branch_at(&ws.main_branch, host) {
                eprintln!("tracefs-daemon: mount live workspace {}: {e:#}", ws.name);
                continue;
            }
        }
        d.watch.add(&ws.id, &d.paths.branch(&ws.main_branch).join("upper"));
        d.watch.touch(&ws.id);
    }
    Ok(())
}
