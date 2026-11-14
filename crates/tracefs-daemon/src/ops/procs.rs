use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{PermissionsExt, chown};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use tracefs_core::merge::{FType, Resolution};
use tracefs_core::path::{self, PathName, RelPath};
use tracefs_core::proto::{ProcChangeset, ProcMerge, ProcRun, ResolutionAction, ResolutionSpec};
use tracefs_core::results::*;

use super::{live, merge_base, merge_into, snap_branches};
use crate::callers::{self, Caller};
use crate::daemon::*;
use crate::helper::RunSpec;
use crate::merging;
use crate::store::{SnapTrees, Undo};
use crate::tree::Tree;

#[derive(Clone)]
pub struct Launch {
    pub env: Vec<String>,
    pub resolver: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Proc {
    pub id: String,
    pub state: String,
    pub cmd: Vec<String>,
    pub cwd: String,
    pub uid: u32,
    pub gid: u32,
    pub pid: Option<i32>,
    pub pid_start: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct PBranch {
    pub ws: Ws,
    pub branch: String,
    pub state: String,
}

fn proc_get(c: &Connection, id: &str) -> Result<Proc> {
    c.query_row("SELECT id, state, cmd, cwd, owner_uid, owner_gid, pid, pid_start, created_at FROM procs WHERE id = ?1", [id], |r| {
        Ok(Proc {
            id: r.get(0)?,
            state: r.get(1)?,
            cmd: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default(),
            cwd: r.get(3)?,
            uid: r.get(4)?,
            gid: r.get(5)?,
            pid: r.get(6)?,
            pid_start: r.get(7)?,
            created_at: r.get(8)?,
        })
    })
    .optional()?
    .ok_or_else(|| not_found("process", id))
}

pub fn proc_for(c: &Connection, uid: u32, id: &str) -> Result<Proc> {
    proc_get(c, id).ok().filter(|p| uid == ROOT || p.uid == uid).ok_or_else(|| not_found("process", id))
}

pub fn branches_of(c: &Connection, id: &str) -> Result<Vec<PBranch>> {
    let ids: Vec<(String, String, String)> =
        rows(c, "SELECT ws, branch, state FROM proc_branches WHERE proc = ?1 ORDER BY ws", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    ids.into_iter().map(|(w, branch, state)| Ok(PBranch { ws: ws_get(c, &w)?, branch, state })).collect()
}

fn set_state(c: &Connection, id: &str, state: &str) -> Result<()> {
    c.execute("UPDATE procs SET state = ?1 WHERE id = ?2", params![state, id])?;
    Ok(())
}

fn set_branch_state(c: &Connection, id: &str, ws: &str, state: &str) -> Result<()> {
    c.execute("UPDATE proc_branches SET state = ?1 WHERE proc = ?2 AND ws = ?3", params![state, id, ws])?;
    Ok(())
}

fn unsettled(b: &PBranch) -> bool {
    b.state == "open" || b.state == "conflicted"
}

fn keys(p: &str, brs: &[PBranch]) -> Vec<String> {
    let mut k = vec![k_proc(p)];
    for b in brs {
        k.push(k_branch(&b.branch));
        k.extend(b.ws.keys());
    }
    k
}

fn lock<'a>(d: &'a Daemon, uid: u32, id: &str) -> Result<(Guard<'a>, Connection, Proc, Vec<PBranch>)> {
    let c0 = d.db()?;
    proc_for(&c0, uid, id)?;
    let g = d.locks.lock(keys(id, &branches_of(&c0, id)?));
    drop(c0);
    let c = d.db()?;
    let p = proc_get(&c, id)?;
    let brs = branches_of(&c, id)?;
    Ok((g, c, p, brs))
}

pub fn unit(d: &Daemon, id: &str) -> String {
    format!("{}-p{id}.scope", d.cfg.slice)
}

fn cgroup(d: &Daemon, id: &str) -> PathBuf {
    Path::new("/sys/fs/cgroup").join(format!("{}.slice", d.cfg.slice)).join(unit(d, id))
}

fn watch_key(branch: &str) -> String {
    format!("b:{branch}")
}

pub fn branch_of_key(key: &str) -> Option<&str> {
    key.strip_prefix("b:")
}

pub struct Frozen(Option<PathBuf>);

impl Drop for Frozen {
    fn drop(&mut self) {
        if let Some(cg) = &self.0
            && let Err(e) = fs::write(cg.join("cgroup.freeze"), "0")
        {
            eprintln!("tracefs-daemon: thaw {}: {e}", cg.display());
        }
    }
}

fn freeze(d: &Daemon, p: &Proc) -> Result<Frozen> {
    let cg = cgroup(d, &p.id);
    if p.state != "running" || !cg.join("cgroup.freeze").exists() {
        return Ok(Frozen(None));
    }
    let events = || fs::read_to_string(cg.join("cgroup.events")).unwrap_or_default();
    let has = |ev: &str, line: &str| ev.lines().any(|l| l == line);
    let ev = events();
    if !has(&ev, "populated 1") || has(&ev, "frozen 1") {
        return Ok(Frozen(None));
    }
    fs::write(cg.join("cgroup.freeze"), "1")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !has(&events(), "frozen 1") {
        if Instant::now() > deadline {
            let _ = fs::write(cg.join("cgroup.freeze"), "0");
            bail!("process {} did not freeze", p.id);
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    Ok(Frozen(Some(cg)))
}

fn stop_scope(d: &Daemon, id: &str) {
    let cg = cgroup(d, id);
    if !cg.join("cgroup.kill").exists() {
        return;
    }
    let _ = fs::write(cg.join("cgroup.freeze"), "0");
    let _ = fs::write(cg.join("cgroup.kill"), "1");
    let deadline = Instant::now() + Duration::from_secs(5);
    while fs::read_to_string(cg.join("cgroup.events")).is_ok_and(|ev| ev.lines().any(|l| l == "populated 1")) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn which(name: &str) -> Result<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .chain(["/usr/bin".into(), "/bin".into()])
        .map(|d| d.join(name))
        .find(|p| p.is_file())
        .ok_or_else(|| anyhow!("{name} not found"))
}

fn caller_groups(pid: i32) -> Vec<u32> {
    let s = fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    s.lines().find_map(|l| l.strip_prefix("Groups:")).map(|g| g.split_whitespace().filter_map(|x| x.parse().ok()).collect()).unwrap_or_default()
}

fn fork_branch<'a>(d: &Daemon, c: &'a Connection, proc_id: &str, main: &Branch, undo: &mut Undo<'a>) -> Result<String> {
    let id = new_id();
    d.fork_subvol(&main.head_snap, &id)?;
    undo.subvol(d.paths.branch(&id));
    c.execute(
        "INSERT INTO branches(id, ws, base, kind, owner, parent_branch, head_snap, fork_snap) VALUES (?1,?2,?3,'process',?4,?5,?6,?6)",
        params![id, main.ws, main.base, proc_id, main.id, main.head_snap],
    )?;
    let row = id.clone();
    undo.defer(move || {
        let _ = c.execute("DELETE FROM branches WHERE id = ?1", [&row]);
    });
    let m = d.mount_branch(&id)?;
    undo.mount(m);
    Ok(id)
}

pub fn start(d: &Daemon, caller: &Caller, a: &ProcRun, stdio: Vec<OwnedFd>) -> Result<(ProcStarted, Child)> {
    if a.cmd.is_empty() {
        return Err(invalid("empty command"));
    }
    if !a.cwd.starts_with('/') {
        return Err(invalid("cwd must be absolute"));
    }
    let [stdin, stdout, stderr]: [OwnedFd; 3] = stdio.try_into().map_err(|_| invalid("proc.run needs stdin, stdout and stderr (SCM_RIGHTS)"))?;
    let uid = caller.uid;
    let wss = {
        let c = d.db()?;
        let mut wss: Vec<Ws> = match a.ws.is_empty() {
            true => live_wss(&c)?.into_iter().filter(|w| w.uid == uid && d.check_ws_path(uid, Path::new(&w.host_path)).is_ok()).collect(),
            false => {
                let wss: Vec<Ws> = a.ws.iter().map(|w| ws_for(&c, uid, w)).collect::<Result<_>>()?;
                for w in &wss {
                    d.check_ws_path(uid, Path::new(&w.host_path))?;
                }
                wss
            }
        };
        wss.sort_by(|x, y| x.id.cmp(&y.id));
        wss.dedup_by(|x, y| x.id == y.id);
        wss
    };
    let id = new_id();
    let _g = d.locks.lock([k_proc(&id)].into_iter().chain(wss.iter().flat_map(Ws::keys)));
    let mut c = d.db()?;
    let mut live = Vec::new();
    for w in &wss {
        let w = ws_get(&c, &w.id)?;
        if !w.live {
            return Err(invalid(format!("{} is detached", w.name)));
        }
        live::sync(d, &mut c, &w, &format!("before process {id}"))?;
        live.push(w);
    }
    let gid = caller.gid;
    let mut undo = Undo::new(d);
    let c = &c;
    c.execute(
        "INSERT INTO procs(id, state, cmd, cwd, owner_uid, owner_gid, created_at) VALUES (?1,'running',?2,?3,?4,?5,?6)",
        params![id, serde_json::to_string(&a.cmd)?, a.cwd, uid, gid, now_ms()],
    )?;
    let row = id.clone();
    undo.defer(move || {
        let _ = c.execute("DELETE FROM procs WHERE id = ?1", [&row]);
        let _ = c.execute("DELETE FROM proc_branches WHERE proc = ?1", [&row]);
    });
    let mut binds = Vec::new();
    for w in &live {
        let b = fork_branch(d, c, &id, &branch_get(c, &w.main_branch)?, &mut undo)?;
        c.execute("INSERT INTO proc_branches(proc, ws, branch) VALUES (?1,?2,?3)", params![id, w.id, b])?;
        binds.push((d.paths.mnt(&b).to_string_lossy().into_owned(), w.host_path.clone()));
    }
    let hide = ["mnt", "views", "resolve"].iter().map(|n| d.paths.run.join(n).to_string_lossy().into_owned()).collect();
    let env: Vec<String> = a.env.iter().filter(|e| !e.starts_with("TRACEFS_PROC=")).cloned().collect();
    let mut proc_env = env.clone();
    proc_env.push(format!("TRACEFS_PROC={id}"));
    let spec = RunSpec { binds, hide, uid, gid, groups: caller_groups(caller.pid), cwd: a.cwd.clone(), cmd: a.cmd.clone(), env: proc_env };
    let child = Command::new(which("systemd-run")?)
        .args(["--scope", "--quiet", "--collect"])
        .arg(format!("--unit={}", unit(d, &id)))
        .arg(format!("--slice={}.slice", d.cfg.slice))
        .arg("--")
        .arg(format!("/proc/{}/exe", std::process::id()))
        .args(["ns-helper", "run"])
        .env_clear()
        .env(crate::helper::SPEC_ENV, serde_json::to_string(&spec)?)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .context("start systemd-run")?;
    let pid = child.id() as i32;
    c.execute("UPDATE procs SET pid = ?1, pid_start = ?2 WHERE id = ?3", params![pid, callers::start_time(pid).map(|s| s as i64), id])?;
    undo.commit();
    d.launches.lock().unwrap().insert(id.clone(), Launch { env, resolver: a.resolver.clone() });
    let brs = branches_of(c, &id)?;
    for b in &brs {
        d.watch.add(&watch_key(&b.branch), &d.paths.branch(&b.branch).join("upper"));
    }
    Ok((ProcStarted { id, pid, branches: brs.iter().map(|b| branch_info(c, b)).collect() }, child))
}

fn branch_info(c: &Connection, b: &PBranch) -> ProcBranch {
    ProcBranch {
        ws: b.ws.name.clone(),
        path: b.ws.host_path.clone(),
        branch: b.branch.clone(),
        head_snap: branch_get(c, &b.branch).map(|x| x.head_snap).unwrap_or_default(),
        state: b.state.clone(),
    }
}

fn snapshot(d: &Daemon, c: &mut Connection, p: &Proc, brs: &[&PBranch], message: &str) -> Result<String> {
    let mut undo = Undo::new(d);
    let taken = snap_branches(d, c, brs.iter().map(|b| b.branch.as_str()), &mut undo)?;
    let snaps: Vec<NewSnap> = taken.into_iter().map(|(b, id)| NewSnap::after(&b, id, None)).collect();
    let cs = new_id();
    record_changeset(c, &NewChangeset::new(cs.clone(), "process", &p.id, "tracefs", message), &snaps)?;
    undo.commit();
    Ok(cs)
}

pub fn changeset(d: &Daemon, uid: u32, a: ProcChangeset) -> Result<ProcChangesetTaken> {
    let (_g, mut c, p, brs) = lock(d, uid, &a.proc_id)?;
    if p.state != "running" {
        return Err(bad_state(format!("process is {}", p.state)));
    }
    let open: Vec<&PBranch> = brs.iter().filter(|b| unsettled(b)).collect();
    if open.is_empty() {
        return Err(bad_state("the process has no workspace branch"));
    }
    let since = match a.since.as_deref() {
        Some(cs) => {
            let snaps = snaps_of_changeset(&c, cs)?;
            if snaps.is_empty() {
                return Err(not_found("changeset", cs));
            }
            snaps
        }
        None => Vec::new(),
    };
    let mut undo = Undo::new(d);
    let t0 = Instant::now();
    let frozen = freeze(d, &p)?;
    let paused = frozen.0.is_some();
    let taken = snap_branches(d, &c, open.iter().map(|b| b.branch.as_str()), &mut undo);
    drop(frozen);
    let pause_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let taken = taken?;
    let mut changed = false;
    for (b, id) in &taken {
        let against = since.iter().find(|s| s.ws == b.ws && s.present).map_or(b.head_snap.as_str(), |s| s.id.as_str());
        if live::changed(d, &c, against, id, &b.base)? {
            changed = true;
            break;
        }
    }
    if !changed {
        return Ok(ProcChangesetTaken { changeset: None, unchanged: true, snaps: Vec::new(), paused, pause_ms });
    }
    let snaps: Vec<NewSnap> = taken.iter().map(|(b, id)| NewSnap::after(b, id.clone(), None)).collect();
    let cs = new_id();
    record_changeset(
        &mut c,
        &NewChangeset { tool_call: a.tool_call.as_deref(), ..NewChangeset::new(cs.clone(), "process", &p.id, &a.author, &a.message) },
        &snaps,
    )?;
    undo.commit();
    let snaps = snaps.into_iter().map(|s| SnapRef { ws: ws_get(&c, &s.ws).map(|w| w.name).unwrap_or_default(), snap: s.id, branch: s.branch }).collect();
    Ok(ProcChangesetTaken { changeset: Some(cs), unchanged: false, snaps, paused, pause_ms })
}

pub fn auto_snapshot(d: &Daemon, branch: &str) -> Result<()> {
    let c0 = d.db()?;
    let Some(proc_id): Option<String> = c0.query_row("SELECT proc FROM proc_branches WHERE branch = ?1", [branch], |r| r.get(0)).optional()? else {
        return Ok(());
    };
    drop(c0);
    let _g = d.locks.lock([k_proc(&proc_id), k_branch(branch)]);
    let mut c = d.db()?;
    let p = proc_get(&c, &proc_id)?;
    if p.state != "running" || !branches_of(&c, &p.id)?.iter().any(|b| b.branch == branch && unsettled(b)) {
        return Ok(());
    }
    let mut undo = Undo::new(d);
    let Some((b, id)) = snap_branches(d, &c, [branch], &mut undo)?.pop() else { return Ok(()) };
    if !live::changed(d, &c, &b.head_snap, &id, &b.base)? {
        return Ok(());
    }
    record_changeset(&mut c, &NewChangeset::new(new_id(), "process", &p.id, "auto", "auto"), &[NewSnap::after(&b, id, None)])?;
    undo.commit();
    Ok(())
}

struct Pending {
    pb: PBranch,
    out: MergeResult,
    o: String,
    s: String,
    t: String,
}

fn merge_branch(d: &Daemon, c: &mut Connection, p: &Proc, pb: &PBranch, res: &BTreeMap<RelPath, Resolution>) -> Result<(MergeResult, bool, Pending)> {
    live::sync(d, c, &pb.ws, &format!("before merging process {}", p.id))?;
    let main = branch_get(c, &pb.ws.main_branch)?;
    let s = branch_get(c, &pb.branch)?.head_snap;
    let o = merge_base(c, &s, &main.head_snap)?;
    let (mut out, ok) = merge_into(d, c, &s, &main, Some(&o), res, ("process", &p.id), &format!("merge process {}", p.id))?;
    out.ws = pb.ws.name.clone();
    out.from = Some(s.clone());
    let pending = Pending { pb: pb.clone(), out: out.clone(), o, s, t: main.head_snap };
    Ok((out, ok, pending))
}

fn discard_branch(d: &Daemon, c: &mut Connection, p: &Proc, pb: &PBranch) -> Result<MergeResult> {
    if p.state == "running" {
        let frozen = freeze(d, p)?;
        snapshot(d, c, p, &[pb], "before discard")?;
        let b = branch_get(c, &pb.branch)?;
        let main = branch_get(c, &pb.ws.main_branch)?;
        let (out, _) = merge_into(d, c, &main.head_snap, &b, Some(&b.head_snap), &BTreeMap::new(), ("process", &p.id), "discard: back to main")?;
        drop(frozen);
        set_branch_state(c, &p.id, &pb.ws.id, "open")?;
        return Ok(MergeResult { ws: pb.ws.name.clone(), status: MergeStatus::Discarded, ..out });
    }
    d.watch.remove(&watch_key(&pb.branch));
    d.drop_branch(c, &pb.branch)?;
    set_branch_state(c, &p.id, &pb.ws.id, "discarded")?;
    Ok(MergeResult::new(&pb.ws.name, MergeStatus::Discarded))
}

fn update(d: &Daemon, c: &mut Connection, p: &Proc, brs: &[&PBranch]) -> Result<Vec<MergeResult>> {
    let mut stale = Vec::new();
    for pb in brs {
        let main = branch_get(c, &pb.ws.main_branch)?;
        let b = branch_get(c, &pb.branch)?;
        if merge_base(c, &main.head_snap, &b.head_snap)? != main.head_snap {
            stale.push(*pb);
        }
    }
    if stale.is_empty() {
        return Ok(Vec::new());
    }
    let _frozen = freeze(d, p)?;
    snapshot(d, c, p, &stale, "before update")?;
    let mut out = Vec::new();
    for pb in stale {
        let main = branch_get(c, &pb.ws.main_branch)?;
        let b = branch_get(c, &pb.branch)?;
        let (mut r, _) = merge_into(d, c, &main.head_snap, &b, None, &BTreeMap::new(), ("process", &p.id), "update from main")?;
        r.ws = pb.ws.name.clone();
        r.from = Some(main.head_snap);
        out.push(r);
    }
    Ok(out)
}

struct MergeOpts {
    resolutions: Vec<ResolutionSpec>,
    discard_ws: Vec<String>,
    resolver: bool,
    message: &'static str,
    only: Option<Vec<String>>,
}

const EXIT: MergeOpts = MergeOpts { resolutions: Vec::new(), discard_ws: Vec::new(), resolver: true, message: "exit", only: None };

fn settle(d: &Daemon, c: &mut Connection, p: &Proc, brs: &[PBranch], o: &MergeOpts) -> Result<(ProcMerged, Vec<Pending>)> {
    let running = p.state == "running";
    if !running && p.state != "conflicted" && p.state != "exiting" {
        return Err(bad_state(format!("process is {}", p.state)));
    }
    let mut results = Vec::new();
    for w in &o.discard_ws {
        let pb = brs
            .iter()
            .find(|b| (b.ws.name == *w || b.ws.id == *w) && unsettled(b))
            .ok_or_else(|| invalid(format!("{w} is not an open branch of this process")))?;
        results.push(discard_branch(d, c, p, pb)?);
    }
    let brs = branches_of(c, &p.id)?;
    let open: Vec<&PBranch> = brs
        .iter()
        .filter(|b| unsettled(b))
        .filter(|b| !o.discard_ws.iter().any(|w| *w == b.ws.name || *w == b.ws.id))
        .filter(|b| o.only.as_ref().is_none_or(|only| only.contains(&b.ws.id)))
        .collect();
    let changeset = match (running, open.is_empty()) {
        (true, false) => {
            let frozen = freeze(d, p)?;
            let r = snapshot(d, c, p, &open, o.message);
            drop(frozen);
            Some(r?)
        }
        _ => None,
    };
    let mut merged = Vec::new();
    let mut pending = Vec::new();
    for pb in &open {
        let res = merging::resolutions_for(&o.resolutions, &pb.ws.id, &pb.ws.name)?;
        let (out, ok, pend) = merge_branch(d, c, p, pb, &res)?;
        results.push(out);
        if ok {
            merged.push(*pb);
        } else {
            set_branch_state(c, &p.id, &pb.ws.id, "conflicted")?;
            pending.push(pend);
        }
    }
    for pb in &merged {
        set_branch_state(c, &p.id, &pb.ws.id, if running { "open" } else { "merged" })?;
    }
    let updated = if running { update(d, c, p, &merged)? } else { Vec::new() };
    if !running {
        for pb in &merged {
            d.watch.remove(&watch_key(&pb.branch));
            d.drop_branch(c, &pb.branch)?;
        }
    }
    let after = branches_of(c, &p.id)?;
    let conflict = after.iter().any(|b| b.state == "conflicted");
    let state = match (running, conflict) {
        (true, _) => "running",
        (false, true) => "conflicted",
        (false, false) if after.iter().all(|b| b.state == "discarded") && !after.is_empty() => "discarded",
        (false, false) => "merged",
    };
    set_state(c, &p.id, state)?;
    if !running && !conflict {
        d.launches.lock().unwrap().remove(&p.id);
        let _ = fs::remove_dir_all(d.paths.resolve(&p.id));
    }
    let resolvable = o.resolver && d.launches.lock().unwrap().get(&p.id).is_some_and(|l| !l.resolver.is_empty());
    if !resolvable {
        pending.clear();
    }
    Ok((ProcMerged { id: p.id.clone(), state: state.into(), changeset, results, updated, conflict, resolver: None }, pending))
}

fn settle_unlocked<'a>(
    d: &'a Daemon,
    id: &str,
    o: MergeOpts,
    first: impl FnOnce() -> Result<(Guard<'a>, Connection, Proc, Vec<PBranch>)>,
) -> Result<ProcMerged> {
    let (mut out, pending, p) = {
        let (_g, mut c, p, brs) = first()?;
        let (out, pending) = settle(d, &mut c, &p, &brs, &o)?;
        (out, pending, p)
    };
    if pending.is_empty() {
        return Ok(out);
    }
    let Some(launch) = d.launches.lock().unwrap().get(id).cloned() else { return Ok(out) };
    let (specs, status) = run_resolver(d, &p, &launch, &pending).unwrap_or_else(|e| (Vec::new(), format!("failed: {e:#}")));
    out.resolver = Some(status);
    if specs.is_empty() {
        return Ok(out);
    }
    let (_g, mut c, p, brs) = lock(d, ROOT, id)?;
    let mut resolutions = o.resolutions;
    resolutions.extend(specs);
    let only: Vec<String> = pending.iter().map(|x| x.pb.ws.id.clone()).collect();
    let again = MergeOpts { resolutions, discard_ws: Vec::new(), resolver: false, message: "resolved", only: Some(only.clone()) };
    let (second, _) = settle(d, &mut c, &p, &brs, &again)?;
    let names: Vec<&String> = pending.iter().map(|x| &x.pb.ws.name).collect();
    out.results.retain(|r| !names.contains(&&r.ws));
    out.results.extend(second.results);
    out.updated.extend(second.updated);
    out.state = second.state;
    out.conflict = second.conflict;
    Ok(out)
}

pub fn merge(d: &Daemon, uid: u32, a: ProcMerge) -> Result<ProcMerged> {
    let o = MergeOpts { resolutions: a.resolutions, discard_ws: a.discard_ws, resolver: !a.no_resolver, message: "task done", only: None };
    let id = a.proc_id.clone();
    settle_unlocked(d, &id, o, || lock(d, uid, &a.proc_id))
}

pub fn exited(d: &Daemon, id: &str) -> Result<ProcMerged> {
    stop_scope(d, id);
    settle_unlocked(d, id, EXIT, || {
        let (g, mut c, p, brs) = lock(d, ROOT, id)?;
        if p.state != "running" {
            return Err(bad_state(format!("process is {}", p.state)));
        }
        for b in &brs {
            d.watch.remove(&watch_key(&b.branch));
        }
        let open: Vec<&PBranch> = brs.iter().filter(|b| unsettled(b)).collect();
        if !open.is_empty() {
            snapshot(d, &mut c, &p, &open, "exit")?;
        }
        for b in &brs {
            d.unmount_branch(&b.branch)?;
        }
        set_state(&c, &p.id, "exiting")?;
        let p = Proc { state: "exiting".into(), ..p };
        Ok((g, c, p, brs))
    })
}

pub fn kill(d: &Daemon, uid: u32, id: &str, signal: Option<i32>) -> Result<ProcKilled> {
    let c = d.db()?;
    let p = proc_for(&c, uid, id)?;
    let sig = signal.unwrap_or(15);
    let s = rustix::process::Signal::from_named_raw(sig).ok_or_else(|| invalid(format!("unknown signal {sig}")))?;
    let sent = match (p.state.as_str(), p.pid, p.pid_start) {
        ("running", Some(pid), Some(start)) if callers::start_time(pid) == Some(start as u64) => {
            rustix::process::Pid::from_raw(pid).is_some_and(|pid| rustix::process::kill_process_group(pid, s).is_ok())
        }
        _ => false,
    };
    Ok(ProcKilled { id: p.id, signal: sig, sent })
}

pub fn signal_group(pid: i32, sig: i32) {
    if let (Some(pid), Some(s)) = (rustix::process::Pid::from_raw(pid), rustix::process::Signal::from_named_raw(sig)) {
        let _ = rustix::process::kill_process_group(pid, s);
    }
}

pub fn cgroup_kill_path(d: &Daemon, id: &str) -> PathBuf {
    cgroup(d, id).join("cgroup.kill")
}

fn info(c: &Connection, p: Proc) -> Result<ProcInfo> {
    let branches = branches_of(c, &p.id)?.iter().map(|b| branch_info(c, b)).collect();
    let pid = if p.state == "running" { p.pid } else { None };
    Ok(ProcInfo { id: p.id, state: p.state, pid, cmd: p.cmd, cwd: p.cwd, created_at: p.created_at, branches })
}

pub fn show(d: &Daemon, uid: u32, id: &str) -> Result<ProcInfo> {
    let c = d.db()?;
    let p = proc_for(&c, uid, id)?;
    info(&c, p)
}

pub fn list(d: &Daemon, uid: u32) -> Result<Vec<ProcInfo>> {
    let c = d.db()?;
    let ids: Vec<String> = col(&c, "SELECT id FROM procs WHERE ?1 = 0 OR owner_uid = ?1 ORDER BY created_at", [uid])?;
    ids.iter().map(|id| info(&c, proc_get(&c, id)?)).collect()
}

pub fn running_proc_of_branch(c: &Connection, branch: &str) -> Result<Option<Proc>> {
    let id: Option<String> = c.query_row("SELECT proc FROM proc_branches WHERE branch = ?1", [branch], |r| r.get(0)).optional()?;
    Ok(id.map(|id| proc_get(c, &id)).transpose()?.filter(|p| p.state == "running"))
}

pub fn frozen_for_write(d: &Daemon, c: &mut Connection, p: &Proc, branch: &str, why: &str) -> Result<Frozen> {
    let frozen = freeze(d, p)?;
    let pb = branches_of(c, &p.id)?.into_iter().find(|b| b.branch == branch).ok_or_else(|| anyhow!("branch {branch} is not the process's"))?;
    snapshot(d, c, p, &[&pb], why)?;
    Ok(frozen)
}

pub fn reset_own(d: &Daemon, c: &mut Connection, p: &Proc, branch: &str, target: &str, message: &str) -> Result<MergeResult> {
    let frozen = frozen_for_write(d, c, p, branch, &format!("before {message}"))?;
    let head = branch_get(c, branch)?.head_snap;
    let mut chain = Vec::new();
    let mut cur = Some(head.clone());
    while let Some(x) = cur.clone().filter(|x| *x != target) {
        let s = snap_get(c, &x)?;
        cur = s.parent.clone();
        chain.push(s);
    }
    let own_history = cur.is_some();
    let mut segments = Vec::new();
    if own_history {
        let mut top: Option<String> = None;
        for s in &chain {
            let incoming = match &s.merge_parent {
                Some(m) => snap_get(c, m)?.branch != branch,
                None => false,
            };
            if !incoming {
                top.get_or_insert_with(|| s.id.clone());
            } else if let Some(t) = top.take() {
                segments.push((t, s.id.clone()));
            }
        }
        if let Some(t) = top {
            segments.push((t, target.to_string()));
        }
    } else {
        segments.push((head, target.to_string()));
    }
    let mut last = MergeResult::new("", MergeStatus::UpToDate);
    let mut actions = 0;
    for (newer, older) in segments {
        let b = branch_get(c, branch)?;
        let (out, ok) = merge_into(d, c, &older, &b, Some(&newer), &BTreeMap::new(), ("process", &p.id), message)?;
        actions += out.actions.unwrap_or(0);
        if !ok {
            drop(frozen);
            return Ok(out);
        }
        if out.status == MergeStatus::Merged {
            last = out;
        }
    }
    drop(frozen);
    if actions > 0 {
        last.actions = Some(actions);
    }
    Ok(last)
}

pub fn recover(d: &Arc<Daemon>) -> Result<()> {
    let c = d.db()?;
    let exiting: Vec<String> = col(&c, "SELECT id FROM procs WHERE state = 'exiting'", [])?;
    for id in exiting {
        let d = d.clone();
        std::thread::spawn(move || match settle_unlocked(&d, &id, EXIT, || lock(&d, ROOT, &id)) {
            Ok(m) => eprintln!("tracefs-daemon: finished merging process {id} after a restart: {}", m.state),
            Err(e) => eprintln!("tracefs-daemon: settle process {id}: {e:#}"),
        });
    }
    let running: Vec<String> = col(&c, "SELECT id FROM procs WHERE state = 'running'", [])?;
    for id in running {
        let p = proc_get(&c, &id)?;
        let alive = p.pid.zip(p.pid_start).and_then(|(pid, start)| callers::pidfd_of(pid, start as u64));
        for b in branches_of(&c, &id)? {
            if alive.is_some() && unsettled(&b) {
                d.mount_branch(&b.branch)?;
                d.watch.add(&watch_key(&b.branch), &d.paths.branch(&b.branch).join("upper"));
            }
        }
        let d = d.clone();
        std::thread::spawn(move || {
            if let Some(fd) = alive {
                callers::wait_exit(&fd);
            }
            match exited(&d, &id) {
                Ok(m) => eprintln!("tracefs-daemon: process {id} exited while tracefs was down or restarting: {}", m.state),
                Err(e) => eprintln!("tracefs-daemon: settle process {id}: {e:#}"),
            }
        });
    }
    Ok(())
}

fn exit_text(st: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (st.code(), st.signal()) {
        (Some(c), _) => format!("exit {c}"),
        (_, Some(s)) => format!("signal {s}"),
        _ => st.to_string(),
    }
}

const MAX_SIDE: u64 = 4 << 20;
const MAX_TOTAL: u64 = 32 << 20;
const MAX_RESULT: u64 = 64 << 20;

const TASK: &str = "# Merge conflicts

Two lines of work changed the same files of a workspace. Decide what each file ends up as.

`conflicts.json` lists them. For each entry:

- `id`: its number; its files are in the directory `<id>/`.
- `ws`, `dir`, `path`: the workspace, its directory and the path in it. The directory shows the
  workspace as it is now (`ours`); read around it for context, but do not change it.
- `reason`: why it conflicts (`content`: both changed the text in overlapping places; `delete/modify`:
  one side deleted what the other changed; `add/add`, `binary`, `type`, ...).
- `base`: the file before either change; `ours`: the file as it is in the workspace now (the user's and
  other agents' work, already merged); `theirs`: the version of the agent whose work is being merged.
  Each is a file in this directory, or null when that side has no regular file at the path.

Write `resolutions.json`: a JSON list of `{\"id\": <id>, \"action\": \"ours\" | \"theirs\" | \"content\"}`.
For `content`, first write the merged file to `<id>/result`. Keep what both sides meant to do. Leave an
id out when you cannot decide: it stays a conflict for a person to look at.

Only write inside this directory.
";

#[derive(Deserialize)]
struct Answer {
    id: usize,
    action: ResolutionAction,
}

fn run_resolver(d: &Daemon, p: &Proc, launch: &Launch, pending: &[Pending]) -> Result<(Vec<ResolutionSpec>, String)> {
    let dir = d.paths.resolve(&p.id);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    chown(&dir, Some(p.uid), Some(p.gid))?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let own = |path: &Path, data: &[u8]| -> Result<()> {
        fs::write(path, data)?;
        chown(path, Some(p.uid), Some(p.gid))?;
        Ok(())
    };
    let c = d.db()?;
    let mut items = Vec::new();
    let mut total = 0u64;
    let mut ids: BTreeMap<usize, (String, PathName)> = BTreeMap::new();
    for pend in pending {
        let sides = [("base", SnapTrees::open(d, &c, &pend.o)?), ("ours", SnapTrees::open(d, &c, &pend.t)?), ("theirs", SnapTrees::open(d, &c, &pend.s)?)];
        for cf in &pend.out.conflicts {
            let n = ids.len() + 1;
            let rel = cf.path.to_bytes().ok_or_else(|| anyhow!("bad conflict path"))?;
            let sub = dir.join(n.to_string());
            fs::create_dir(&sub)?;
            chown(&sub, Some(p.uid), Some(p.gid))?;
            let mut item = serde_json::json!({ "id": n, "ws": pend.pb.ws.name, "dir": pend.pb.ws.host_path, "path": cf.path, "reason": cf.reason });
            for (name, side) in &sides {
                let st = side.view.lstat(&rel).ok().flatten().filter(|s| s.ftype() == FType::Reg && s.size <= MAX_SIDE);
                let file = match st {
                    Some(s) if total + s.size <= MAX_TOTAL => {
                        total += s.size;
                        own(&sub.join(name), &side.view.read_all(&rel)?)?;
                        Some(format!("{n}/{name}"))
                    }
                    _ => None,
                };
                item[*name] = serde_json::json!(file);
            }
            items.push(item);
            ids.insert(n, (pend.pb.ws.name.clone(), cf.path.clone()));
        }
    }
    own(&dir.join("conflicts.json"), &serde_json::to_vec_pretty(&items)?)?;
    own(&dir.join("TASK.md"), TASK.as_bytes())?;
    let log = fs::File::create(dir.join("log"))?;
    chown(dir.join("log"), Some(p.uid), Some(p.gid))?;
    let mut cmd = Command::new(&launch.resolver[0]);
    cmd.args(&launch.resolver[1..])
        .env_clear()
        .envs(launch.env.iter().filter_map(|e| e.split_once('=')))
        .env("TRACEFS_RESOLVE_DIR", &dir)
        .current_dir(&dir)
        .uid(p.uid)
        .gid(p.gid)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    let mut child = cmd.spawn().with_context(|| format!("start resolver {}", launch.resolver[0]))?;
    let deadline = Instant::now() + Duration::from_secs(d.cfg.resolver_timeout_secs);
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() > deadline {
            signal_group(child.id() as i32, 9);
            let _ = child.wait();
            return Ok((Vec::new(), format!("timed out after {} s", d.cfg.resolver_timeout_secs)));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    signal_group(child.id() as i32, 9);
    let tree = Tree::open_ro(&dir)?;
    let answers: Vec<Answer> = match tree.lstat(b"resolutions.json")? {
        Some(s) if s.ftype() == FType::Reg && s.size <= 1 << 20 => {
            serde_json::from_slice(&tree.read_all(b"resolutions.json")?).map_err(|e| anyhow!("resolutions.json: {e}"))?
        }
        _ => return Ok((Vec::new(), format!("{}, no resolutions.json", exit_text(status)))),
    };
    let mut specs = Vec::new();
    for a in answers {
        let Some((ws, path)) = ids.get(&a.id) else { continue };
        let content = match a.action {
            ResolutionAction::Content => {
                let rel = format!("{}/result", a.id).into_bytes();
                match tree.lstat(&rel)? {
                    Some(s) if s.ftype() == FType::Reg && s.size <= MAX_RESULT => Some(path::b64_encode(&tree.read_all(&rel)?)),
                    _ => continue,
                }
            }
            _ => None,
        };
        specs.push(ResolutionSpec { ws: ws.clone(), path: path.clone(), action: a.action, content });
    }
    let status = format!("resolved {} of {} conflicts ({})", specs.len(), ids.len(), exit_text(status));
    let mut f = fs::OpenOptions::new().append(true).open(dir.join("log"))?;
    let _ = writeln!(f, "tracefs: {status}");
    Ok((specs, status))
}
