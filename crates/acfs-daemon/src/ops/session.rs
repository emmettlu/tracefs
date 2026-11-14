use std::os::fd::OwnedFd;

use acfs_core::proto::SessionCreate;
use acfs_core::results::{LiveSpan, MergeStatus, SessionCreated, SessionEnded, SessionInfo};
use anyhow::Result;
use rusqlite::params;

use super::live;
use crate::callers::{self, Caller};
use crate::daemon::*;

fn caller_process(caller: &Caller) -> Result<(OwnedFd, u64)> {
    let fd = caller.pidfd().map_err(|e| invalid(format!("end_on_exit needs SO_PEERPIDFD (Linux 6.5): {e}")))?;
    let start = callers::start_time(caller.pid).filter(|_| !callers::exited(&fd)).ok_or_else(|| invalid("the calling process has exited"))?;
    Ok((fd, start))
}

pub fn create(d: &Daemon, caller: &Caller, a: SessionCreate) -> Result<SessionCreated> {
    let uid = caller.uid;
    let bound = if a.end_on_exit { Some(caller_process(caller)?) } else { None };
    let name = a.name;
    if let Some(n) = &name
        && !acfs_core::names::valid_ws_name(n)
    {
        return Err(invalid(format!("invalid session name {n:?}")));
    }
    let wss = {
        let c = d.db()?;
        a.ws.iter().map(|w| ws_for(&c, uid, w)).collect::<Result<Vec<_>>>()?
    };
    let keys = wss.iter().flat_map(Ws::keys);
    let _g = d.locks.lock(["sessions".to_string()].into_iter().chain(keys));
    let mut c = d.db()?;
    for w in &wss {
        if !ws_get(&c, &w.id)?.live {
            return Err(invalid(format!("{} is detached", w.name)));
        }
    }
    if let Some(n) = &name
        && c.query_row("SELECT 1 FROM sessions WHERE name = ?1 AND state = 'active'", [n], |_| Ok(())).is_ok()
    {
        return Err(api("exists", format!("an active session named {n:?} exists")));
    }
    if uid != ROOT {
        let n: u32 = c.query_row("SELECT COUNT(*) FROM sessions WHERE state = 'active' AND owner_uid = ?1", [uid], |r| r.get(0))?;
        if n >= d.cfg.max_user_sessions {
            return Err(api("exhausted", format!("uid {uid} has {n} active sessions (limit {})", d.cfg.max_user_sessions)));
        }
    }
    let id = new_id();
    let idle = a.idle_timeout.map(|s| s.saturating_mul(1000) as i64);
    c.execute(
        "INSERT INTO sessions(id, name, state, created_at, idle_timeout_ms, owner_uid, caller_pid, caller_start) VALUES (?1,?2,'active',?3,?4,?5,?6,?7)",
        params![id, name, now_ms(), idle, uid, bound.as_ref().map(|_| caller.pid), bound.as_ref().map(|b| b.1 as i64)],
    )?;
    let live = match live::label(d, &mut c, &id, &wss) {
        Ok(v) => v,
        Err(e) => {
            let _ = c.execute("DELETE FROM session_live WHERE session = ?1", [&id]);
            let _ = c.execute("DELETE FROM sessions WHERE id = ?1", [&id]);
            return Err(e);
        }
    };
    if let Some((fd, _)) = bound {
        d.callers.bind(&id, fd);
    }
    Ok(SessionCreated { id, name, live })
}

pub fn list(d: &Daemon, uid: u32) -> Result<Vec<SessionInfo>> {
    let c = d.db()?;
    let sql = "SELECT id, name, state, created_at FROM sessions WHERE ?1 = 0 OR owner_uid = ?1 ORDER BY created_at";
    let mut out: Vec<SessionInfo> =
        rows(&c, sql, [uid], |r| Ok(SessionInfo { id: r.get(0)?, name: r.get(1)?, state: r.get(2)?, created_at: r.get(3)?, live: Vec::new() }))?;
    let sql = "SELECT w.name, l.start_snap, l.end_snap FROM session_live l JOIN workspaces w ON w.id = l.ws WHERE l.session = ?1 ORDER BY w.name";
    for s in &mut out {
        s.live = rows(&c, sql, [&s.id], |r| Ok(LiveSpan { ws: r.get(0)?, start_snap: r.get(1)?, end_snap: r.get(2)?, discard: None }))?;
    }
    Ok(out)
}

pub fn end(d: &Daemon, uid: u32, session: &str, discard: bool) -> Result<SessionEnded> {
    let c0 = d.db()?;
    let sess = session_for(&c0, uid, session)?;
    let keys: Vec<String> = live::session_wss(&c0, &sess.id)?.iter().flat_map(Ws::keys).collect();
    drop(c0);
    let _g = d.locks.lock([k_session(&sess.id)].into_iter().chain(keys));
    let mut c = d.db()?;
    let sess = session_get(&c, &sess.id)?;
    sess.ensure_active()?;
    let live = live::unlabel(d, &mut c, &sess.id, discard)?;
    let state = if discard { "discarded" } else { "merged" };
    c.execute("UPDATE sessions SET state = ?1 WHERE id = ?2", params![state, sess.id])?;
    d.callers.forget(&sess.id);
    let conflict = live.iter().any(|l| l.discard.as_ref().is_some_and(|r| r.status == MergeStatus::Conflict));
    Ok(SessionEnded { session: sess.id, state: state.into(), live, conflict })
}

fn active(c: &rusqlite::Connection) -> Result<Vec<(String, Option<i64>, i64)>> {
    let sql = "SELECT s.id, s.idle_timeout_ms, MAX(s.created_at, IFNULL((SELECT MAX(created_at) FROM snaps WHERE session = s.id), 0))
               FROM sessions s WHERE s.state = 'active'";
    rows(c, sql, [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
}

fn finish(d: &Daemon, session: &str, why: &str) {
    match end(d, ROOT, session, false) {
        Ok(_) => eprintln!("acfs-daemon: ended session {session} ({why})"),
        Err(e) => eprintln!("acfs-daemon: end session {session} ({why}): {e:#}"),
    }
}

pub fn end_idle(d: &Daemon) {
    let gone = d.callers.gone();
    let Ok(sessions) = d.db().and_then(|c| active(&c)) else { return };
    let now = now_ms();
    for (id, timeout, last) in sessions {
        let exited = gone.contains(&id);
        if !exited && timeout.is_none_or(|t| now - last < t) {
            continue;
        }
        let pending = d.db().and_then(|c| live::session_wss(&c, &id)).is_ok_and(|wss| wss.iter().any(|w| d.watch.is_dirty(&w.id)));
        if !pending {
            finish(d, &id, if exited { "caller exited" } else { "idle" });
        }
    }
}

pub fn end_after_boot(d: &Daemon) -> Result<()> {
    for (id, _, _) in active(&d.db()?)? {
        finish(d, &id, "reboot");
    }
    Ok(())
}
