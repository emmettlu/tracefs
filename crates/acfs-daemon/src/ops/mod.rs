pub mod fs;
pub mod history;
pub mod live;
pub mod procs;
pub mod remote;
pub mod retention;
pub mod session;
pub mod ws;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use acfs_core::graph;
use acfs_core::merge::Resolution;
use acfs_core::path::{PathName, RelPath};
use acfs_core::results::{ConflictInfo, MergeResult, MergeStatus};
use anyhow::{Result, anyhow};
use rusqlite::Connection;

use crate::daemon::*;
use crate::merging::{self, Outcome};
use crate::store::Undo;
use crate::sys;

pub fn snap_branches<'a, 'b>(d: &'a Daemon, c: &Connection, branches: impl IntoIterator<Item = &'b str>, undo: &mut Undo<'a>) -> Result<Vec<(Branch, String)>> {
    let mut taken = Vec::new();
    for id in branches {
        let b = branch_get(c, id)?;
        let snap = new_id();
        d.snap_branch(&b.id, &snap)?;
        undo.subvol(d.paths.snap(&snap));
        taken.push((b, snap));
    }
    Ok(taken)
}

pub fn merge_base(c: &Connection, a: &str, b: &str) -> Result<String> {
    let g: HashMap<String, (Vec<String>, bool)> =
        rows(c, "SELECT id, parent, merge_parent, present FROM snaps WHERE ws = (SELECT ws FROM snaps WHERE id = ?1)", [a], |r| {
            Ok((r.get(0)?, ([r.get::<_, Option<String>>(1)?, r.get(2)?].into_iter().flatten().collect(), r.get::<_, i64>(3)? != 0)))
        })?;
    let parents = |n: &str| g.get(n).map_or_else(|| snap_parents(c, n), |(p, _)| p.clone());
    let present = |n: &str| g.get(n).map_or_else(|| snap_present(c, n), |(_, p)| *p);
    graph::merge_base(a, b, parents, present).ok_or_else(|| anyhow!("no common ancestor for {a} and {b}"))
}

pub fn lineage(c: &Connection, a: &str, b: &str) -> Result<bool> {
    let parents: HashMap<String, Option<String>> =
        rows(c, "SELECT id, parent FROM snaps WHERE ws = (SELECT ws FROM snaps WHERE id = ?1)", [a], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let reaches = |from: &str, to: &str| {
        let mut cur = Some(from.to_string());
        for _ in 0..=parents.len() {
            match cur {
                Some(x) if x == to => return true,
                Some(x) => cur = parents.get(&x).cloned().flatten(),
                None => break,
            }
        }
        false
    };
    Ok(reaches(a, b) || reaches(b, a))
}

#[allow(clippy::too_many_arguments)]
pub fn merge_into(
    d: &Daemon,
    c: &mut Connection,
    s: &str,
    target: &Branch,
    base: Option<&str>,
    res: &BTreeMap<RelPath, Resolution>,
    cs_owner: (&str, &str),
    message: &str,
) -> Result<(MergeResult, bool)> {
    let live_ws = live_ws_of_branch(c, &target.id)?;
    let session = ws_session(c, &target.ws)?;
    let mounted = live_ws.is_some() || sys::is_mountpoint(&d.paths.mnt(&target.id));
    let target = &branch_get(c, &target.id)?;
    let t = target.head_snap.clone();
    let o = match base {
        Some(o) => o.to_string(),
        None => merge_base(c, s, &t)?,
    };
    let live = match &live_ws {
        Some(ws) => PathBuf::from(&ws.host_path),
        None => d.mount_branch(&target.id)?,
    };
    journal_begin(c, &target.id, &t, "merge")?;
    let r = (|| -> Result<(MergeResult, bool)> {
        match merging::merge(d, c, &o, s, &t, &live, res)? {
            Outcome::UpToDate => Ok((MergeResult::new("", MergeStatus::UpToDate), true)),
            Outcome::Conflicts(cf, unused) => {
                let mut out = MergeResult::new("", MergeStatus::Conflict);
                out.conflicts = cf.iter().map(|c| ConflictInfo { path: PathName::new(&c.path), reason: c.reason.into() }).collect();
                out.unused_resolutions = merging::path_names(&unused);
                Ok((out, false))
            }
            Outcome::Applied { actions, unused } => {
                crate::failpoint("merge-before-snapshot");
                let snap = new_id();
                d.snap_branch(&target.id, &snap)?;
                let cs = new_id();
                record_changeset(
                    c,
                    &NewChangeset { session: session.as_deref(), ..NewChangeset::new(cs.clone(), cs_owner.0, cs_owner.1, "acfs", message) },
                    &[NewSnap::after(target, snap.clone(), Some(s.to_string()))],
                )?;
                let mut out = MergeResult::new("", MergeStatus::Merged);
                out.actions = Some(actions);
                out.changeset = Some(cs);
                out.snap = Some(snap);
                out.unused_resolutions = merging::path_names(&unused);
                Ok((out, true))
            }
        }
    })();
    match &r {
        Ok(_) => {
            journal_end(c, &target.id)?;
            if !mounted {
                d.unmount_branch(&target.id)?;
            }
        }
        Err(_) => {
            let _ = crate::recover::restore_branch(d, c, &target.id);
        }
    }
    r
}
