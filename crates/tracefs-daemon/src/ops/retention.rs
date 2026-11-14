use std::collections::{BTreeMap, HashSet};

use anyhow::Result;
use rusqlite::{Connection, params};
use tracefs_core::results::Pruned;

use crate::daemon::*;
use crate::diff;
use crate::store::SnapTrees;

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

fn pinned(c: &Connection, ws: &Ws) -> Result<HashSet<String>> {
    let mut out: HashSet<String> = HashSet::new();
    for sql in [
        "SELECT head_snap FROM branches WHERE alive = 1 AND ws = ?1",
        "SELECT fork_snap FROM branches WHERE alive = 1 AND ws = ?1 AND fork_snap IS NOT NULL",
        "SELECT start_snap FROM session_live WHERE ws = ?1",
        "SELECT end_snap FROM session_live WHERE ws = ?1 AND end_snap IS NOT NULL",
        "SELECT restore_snap FROM journal WHERE branch IN (SELECT id FROM branches WHERE ws = ?1)",
    ] {
        out.extend(col::<String, Vec<String>>(c, sql, [&ws.id])?);
    }
    Ok(out)
}

fn select(c: &Connection, d: &Daemon, ws: &Ws, now: i64) -> Result<Vec<String>> {
    let all = d.cfg.retain_all_hours as i64 * HOUR;
    let hourly = d.cfg.retain_hourly_days as i64 * DAY;
    let keep = pinned(c, ws)?;
    let sql = "SELECT s.id, s.created_at FROM snaps s JOIN changesets cs ON cs.id = s.changeset
               WHERE s.ws = ?1 AND s.present = 1 AND (
                 (s.branch = ?2 AND cs.owner_kind = 'workspace' AND cs.author = 'auto') OR
                 (s.branch IN (SELECT id FROM branches WHERE ws = ?1 AND kind = 'process' AND alive = 0)))
               ORDER BY s.created_at, s.id";
    let autos: Vec<(String, i64)> = rows(c, sql, params![ws.id, ws.main_branch], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let mut newest: BTreeMap<(bool, i64), String> = BTreeMap::new();
    for (id, at) in &autos {
        let age = now - at;
        if age >= all {
            let bucket = if age < hourly { (false, at.div_euclid(HOUR)) } else { (true, at.div_euclid(DAY)) };
            newest.insert(bucket, id.clone());
        }
    }
    let kept: HashSet<&String> = newest.values().collect();
    Ok(autos.iter().filter(|(id, at)| now - at >= all && !kept.contains(id) && !keep.contains(id)).map(|(id, _)| id.clone()).collect())
}

pub fn summarize(d: &Daemon, c: &Connection, snap: &Snap) -> Result<()> {
    if snap.summary.is_some() {
        return Ok(());
    }
    let Some(parent) = snap.parent.as_deref().filter(|p| snap_present(c, p)) else { return Ok(()) };
    let (a, b) = (SnapTrees::open(d, c, parent)?, SnapTrees::open(d, c, &snap.id)?);
    let ch = diff::filtered(&a, &b)?;
    c.execute("UPDATE snaps SET summary = ?1 WHERE id = ?2", params![serde_json::to_string(&diff::to_api(&ch))?, snap.id])?;
    Ok(())
}

fn drop_all(d: &Daemon, c: &Connection, ids: &[String]) -> Result<()> {
    for id in ids {
        let s = snap_get(c, id)?;
        summarize(d, c, &s)?;
        let children: Vec<String> = col(c, "SELECT id FROM snaps WHERE parent = ?1 AND present = 1", [id])?;
        for ch in children {
            summarize(d, c, &snap_get(c, &ch)?)?;
        }
        d.delete_subvol(&d.paths.snap(id))?;
        c.execute("UPDATE snaps SET present = 0 WHERE id = ?1", [id])?;
    }
    Ok(())
}

pub fn prune(d: &Daemon, uid: u32, ws_key: &str, dry_run: bool) -> Result<Pruned> {
    let ws = ws_for(&d.db()?, uid, ws_key)?;
    let _g = d.locks.lock(ws.keys());
    let c = d.db()?;
    let ws = ws_get(&c, &ws.id)?;
    let dropped = select(&c, d, &ws, now_ms())?;
    if !dry_run {
        drop_all(d, &c, &dropped)?;
    }
    Ok(Pruned { ws: ws.name, dropped, dry_run })
}

pub fn prune_live(d: &Daemon) {
    let Ok(wss) = d.db().and_then(|c| live_wss(&c)) else { return };
    for ws in wss {
        match prune(d, ROOT, &ws.id, false) {
            Ok(p) if !p.dropped.is_empty() => eprintln!("tracefs-daemon: thinned {} snapshots of {}", p.dropped.len(), ws.name),
            Ok(_) => {}
            Err(e) => eprintln!("tracefs-daemon: thin {}: {e:#}", ws.name),
        }
    }
}
