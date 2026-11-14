use std::fs;
use std::process::Command;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use tracefs_core::merge::{ChangeKind, FType};
use tracefs_core::proto::{Diff, Log};
use tracefs_core::results::{ChangesetInfo, DiffResult, SnapInfo};

use crate::daemon::*;
use crate::diff::{self, Changes};
use crate::merging;
use crate::store::SnapTrees;
use crate::tree::Tree;

pub fn log(d: &Daemon, uid: u32, a: Log) -> Result<Vec<ChangesetInfo>> {
    let c = d.db()?;
    let n = a.n.unwrap_or(50) as i64;
    let ids: Vec<String> = if let Some(p) = &a.proc_id {
        super::procs::proc_for(&c, uid, p)?;
        col(&c, "SELECT id FROM changesets WHERE owner_kind = 'process' AND owner = ?1 ORDER BY id DESC LIMIT ?2", params![p, n])?
    } else if let Some(s) = &a.session {
        let s = session_for(&c, uid, s)?;
        let sql = "SELECT id FROM changesets WHERE session = ?1 OR id IN (SELECT changeset FROM snaps WHERE session = ?1) ORDER BY id DESC LIMIT ?2";
        col(&c, sql, params![s.id, n])?
    } else if let Some(w) = &a.ws {
        let ws = ws_for(&c, uid, w)?;
        col(&c, "SELECT DISTINCT changeset FROM snaps WHERE ws = ?1 ORDER BY changeset DESC LIMIT ?2", params![ws.id, n])?
    } else {
        return Err(invalid("log needs --proc, --session or --ws"));
    };
    ids.iter().map(|id| changeset_info(&c, id)).collect()
}

fn changeset_info(c: &Connection, id: &str) -> Result<ChangesetInfo> {
    let mut cs = c.query_row("SELECT owner_kind, owner, author, message, tool_call, created_at, session FROM changesets WHERE id = ?1", [id], |r| {
        Ok(ChangesetInfo {
            id: id.into(),
            owner_kind: r.get(0)?,
            owner: r.get(1)?,
            author: r.get(2)?,
            message: r.get(3)?,
            tool_call: r.get(4)?,
            created_at: r.get(5)?,
            session: r.get(6)?,
            snaps: Vec::new(),
        })
    })?;
    cs.snaps = snaps_of_changeset(c, id)?
        .into_iter()
        .map(|s| SnapInfo {
            ws: ws_get(c, &s.ws).map(|w| w.name).unwrap_or_default(),
            snap: s.id,
            branch: s.branch,
            parent: s.parent,
            merge_parent: s.merge_parent,
            present: s.present,
        })
        .collect();
    Ok(cs)
}

pub fn resolve(c: &Connection, id: &str) -> Result<Vec<Snap>> {
    if let Ok(s) = snap_get(c, id) {
        return Ok(vec![s]);
    }
    let exists: Option<i64> = c.query_row("SELECT 1 FROM changesets WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    if exists.is_some() {
        return snaps_of_changeset(c, id);
    }
    Err(not_found("changeset or snapshot", id))
}

const PATCH_LIMIT: u64 = 1 << 20;

fn patch(d: &Daemon, ta: Option<&Tree>, tb: &Tree, ch: &Changes) -> Result<String> {
    let dir = d.paths.tmp().join(format!("patch-{}", crate::tree::nanos()));
    fs::create_dir_all(&dir)?;
    let mut out = String::new();
    let read = |t: Option<&Tree>, p: &[u8]| -> Result<Option<Vec<u8>>> {
        let Some(t) = t else { return Ok(Some(Vec::new())) };
        match t.lstat(p)? {
            Some(s) if s.ftype() == FType::Reg && s.size <= PATCH_LIMIT => Ok(Some(t.read_all(p)?)),
            Some(s) if s.ftype() == FType::Reg => Ok(None),
            _ => Ok(Some(Vec::new())),
        }
    };
    for (p, k) in &ch.kinds {
        let is_reg = |t: Option<&Tree>| t.and_then(|t| t.lstat(p).ok().flatten()).is_some_and(|s| s.ftype() == FType::Reg);
        if !is_reg(ta) && !is_reg(Some(tb)) {
            continue;
        }
        let (old, new) = match k {
            ChangeKind::Delete => (read(ta, p)?, Some(Vec::new())),
            ChangeKind::Add => (Some(Vec::new()), read(Some(tb), p)?),
            _ => (read(ta, p)?, read(Some(tb), p)?),
        };
        let name = String::from_utf8_lossy(p);
        let (Some(old), Some(new)) = (old, new) else {
            out.push_str(&format!("Large file {name} differs\n"));
            continue;
        };
        if old.contains(&0) || new.contains(&0) {
            out.push_str(&format!("Binary file {name} differs\n"));
            continue;
        }
        fs::write(dir.join("a"), &old)?;
        fs::write(dir.join("b"), &new)?;
        let la = if *k == ChangeKind::Add { "/dev/null".to_string() } else { format!("a/{name}") };
        let lb = if *k == ChangeKind::Delete { "/dev/null".to_string() } else { format!("b/{name}") };
        let r = Command::new("diff").args(["-u", "--label", &la, "--label", &lb]).arg(dir.join("a")).arg(dir.join("b")).output()?;
        out.push_str(&String::from_utf8_lossy(&r.stdout));
    }
    let _ = fs::remove_dir_all(&dir);
    Ok(out)
}

fn diff_pair(d: &Daemon, c: &Connection, a: Option<&Snap>, b: &Snap, want_patch: bool, (full, all): (bool, bool)) -> Result<DiffResult> {
    let ws = ws_get(c, &b.ws)?;
    if !b.present || a.is_some_and(|a| !a.present) {
        if let (Some(sum), true) = (&b.summary, a.map(|a| Some(&a.id) == b.parent.as_ref()).unwrap_or(false))
            && !want_patch
        {
            let changes = serde_json::from_str(sum)?;
            return Ok(DiffResult { ws: ws.name, from: a.map(|a| a.id.clone()), to: b.id.clone(), changes, summary_only: true, patch: None });
        }
        return Err(bad_state(format!("snapshot data for {} was pruned", b.id)));
    }
    let tb = SnapTrees::open(d, c, &b.id)?;
    let ta = a.map(|a| SnapTrees::open(d, c, &a.id)).transpose()?;
    let mut ch = diff::snaps(ta.as_ref(), &tb, full)?;
    if !all {
        ch = ch.without_ignored(&merging::rules_of(&tb.view), ta.as_ref().map(|t| &t.view), &tb.view);
    }
    if let (Some(a), Some(ta)) = (a, &ta)
        && super::lineage(c, &a.id, &b.id)?
    {
        ch = ch.with_identity_renames(&ta.side(), &tb.side());
    }
    let patch = if want_patch { Some(patch(d, ta.as_ref().map(|t| &t.view), &tb.view, &ch)?) } else { None };
    Ok(DiffResult { ws: ws.name, from: a.map(|a| a.id.clone()), to: b.id.clone(), changes: diff::to_api(&ch), summary_only: false, patch })
}

pub fn diff(d: &Daemon, uid: u32, a: Diff) -> Result<Vec<DiffResult>> {
    let c = d.db()?;
    let left = resolve(&c, &a.a)?;
    check_snaps(&c, uid, &a.a, &left)?;
    let mut out = Vec::new();
    match &a.b {
        None => {
            for s in &left {
                let parent = s.parent.as_deref().map(|p| snap_get(&c, p)).transpose()?;
                out.push(diff_pair(d, &c, parent.as_ref(), s, a.patch, (a.full, a.all))?);
            }
        }
        Some(b) => {
            let right = resolve(&c, b)?;
            check_snaps(&c, uid, b, &right)?;
            for r in &right {
                let Some(l) = left.iter().find(|l| l.ws == r.ws) else { continue };
                out.push(diff_pair(d, &c, Some(l), r, a.patch, (a.full, a.all))?);
            }
            if out.is_empty() {
                return Err(invalid("the two sides share no workspace"));
            }
        }
    }
    Ok(out)
}
