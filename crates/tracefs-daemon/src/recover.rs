use std::collections::HashSet;
use std::fs;

use anyhow::Result;
use rusqlite::Connection;

use crate::daemon::*;
use crate::sys;

pub fn restore_branch(d: &Daemon, c: &mut Connection, branch: &str) -> Result<()> {
    let (snap, op): (String, String) = match c.query_row("SELECT restore_snap, op FROM journal WHERE branch = ?1", [branch], |r| Ok((r.get(0)?, r.get(1)?))) {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };
    if let Some(ws) = live_ws_of_branch(c, branch)? {
        journal_end(c, branch)?;
        let cs = crate::ops::live::sync(d, c, &ws, &format!("interrupted {op}; tracefs reset {snap} undoes it"))?;
        eprintln!("tracefs-daemon: {op} into live workspace {} was interrupted; recorded as {cs:?}, `tracefs reset {snap}` undoes it", ws.name);
        return Ok(());
    }
    if let Some(p) = crate::ops::procs::running_proc_of_branch(c, branch)? {
        eprintln!("tracefs-daemon: {op} into process {}'s branch was interrupted; stopping the process", p.id);
        let _ = fs::write(crate::ops::procs::cgroup_kill_path(d, &p.id), "1");
    }
    d.reset_branch(branch, &snap)?;
    c.execute("UPDATE branches SET head_snap = ?1 WHERE id = ?2", rusqlite::params![snap, branch])?;
    journal_end(c, branch)?;
    eprintln!("tracefs-daemon: restored branch {branch} to {snap}");
    Ok(())
}

fn subvol_names(dir: &std::path::Path) -> Vec<String> {
    fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
}

pub fn run(d: &Daemon) -> Result<()> {
    let mut c = d.db()?;
    for v in subvol_names(&d.paths.views()) {
        let _ = sys::unmount_and_remove(&d.paths.views().join(&v));
    }
    crate::ops::live::remount_all(d)?;
    let journal: Vec<(String, String)> = rows(&c, "SELECT branch, op FROM journal", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
    for (b, _) in journal {
        if let Err(e) = restore_branch(d, &mut c, &b) {
            eprintln!("tracefs-daemon: restore {b}: {e:#}");
        }
    }
    let idle: Vec<String> = col(
        &c,
        "SELECT id FROM branches WHERE alive = 1 AND (kind = 'main' OR id NOT IN (SELECT b.branch FROM proc_branches b JOIN procs p ON p.id = b.proc WHERE p.state = 'running'))",
        [],
    )?;
    for b in idle {
        let _ = d.unmount_branch(&b);
    }
    for dir in fs::read_dir(d.paths.run.join("resolve")).into_iter().flatten().flatten() {
        let _ = fs::remove_dir_all(dir.path());
    }
    let known = |sql: &str| -> Result<HashSet<String>> { col(&c, sql, []) };
    let snaps = known("SELECT id FROM snaps WHERE present = 1")?;
    let branches = known("SELECT id FROM branches WHERE alive = 1")?;
    let bases = known("SELECT id FROM bases")?;
    for (dir, set) in [("snaps", &snaps), ("branches", &branches), ("bases", &bases)] {
        for n in subvol_names(&d.paths.state.join(dir)) {
            if !set.contains(&n) {
                let p = d.paths.state.join(dir).join(&n);
                eprintln!("tracefs-daemon: removing orphan {}", p.display());
                if let Err(e) = d.delete_subvol(&p) {
                    eprintln!("tracefs-daemon: {e:#}");
                }
            }
        }
    }
    let _ = fs::remove_dir_all(d.paths.tmp());
    fs::create_dir_all(d.paths.tmp())?;
    Ok(())
}
