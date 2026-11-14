use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::types::FromSql;
use rusqlite::{Connection, OptionalExtension, Params, Row, params};

use crate::callers::Callers;
use crate::ops::procs::Launch;
use crate::watch::Watcher;

#[derive(Parser, Debug)]
#[command(name = "acfsd", version, about = "AgentComputerFS (acfs) daemon")]
pub struct Config {
    #[arg(help = "State directory (btrfs; otherwise a loop btrfs image <dir>.img is created)")]
    #[arg(long, default_value = "/var/lib/acfs")]
    pub state_dir: PathBuf,
    #[arg(help = "Runtime directory (a private tmpfs is mounted here)")]
    #[arg(long, default_value = "/run/acfs")]
    pub run_dir: PathBuf,
    #[arg(help = "Socket path (default: <run-dir>/acfs-daemon.sock)")]
    #[arg(long)]
    pub socket: Option<PathBuf>,
    #[arg(
        help = "Socket mode (octal). The socket is root:root; every request is authorized by the caller's uid (SO_PEERCRED), so anyone may connect by default"
    )]
    #[arg(long, default_value = "666", value_parser = |s: &str| u32::from_str_radix(s, 8))]
    pub socket_mode: u32,
    #[arg(help = "Active sessions per non-root caller")]
    #[arg(long, default_value_t = 16)]
    pub max_user_sessions: u32,
    #[arg(help = "Slice name; each process runs in the scope <slice>-p<id>.scope below <slice>.slice")]
    #[arg(long, default_value = "acfs")]
    pub slice: String,
    #[arg(help = "Longest a resolver may run (seconds)")]
    #[arg(long, default_value_t = 1800)]
    pub resolver_timeout_secs: u64,
    #[arg(help = "Default remote directory for push/pull (the only one non-root callers may use)")]
    #[arg(long)]
    pub remote_dir: Option<PathBuf>,
    #[arg(help = "Size of the loop image used when the state dir is not on btrfs")]
    #[arg(long, default_value = "64G")]
    pub image_size: String,
    #[arg(long, default_value_t = 10_000)]
    pub stop_timeout_ms: u64,
    #[arg(help = "Base layer count above which promote flattens")]
    #[arg(long, default_value_t = 16)]
    pub max_layers: usize,
    #[arg(help = "Snapshot a workspace or process branch after this long without writes")]
    #[arg(long, default_value_t = 2000)]
    pub auto_quiet_ms: u64,
    #[arg(help = "Snapshot at least this often while writes continue")]
    #[arg(long, default_value_t = 30_000)]
    pub auto_max_ms: u64,
    #[arg(help = "Keep every automatic snapshot of main this long, then one per hour")]
    #[arg(long, default_value_t = 24)]
    pub retain_all_hours: u64,
    #[arg(help = "Keep one automatic snapshot of main per hour this long, then one per day")]
    #[arg(long, default_value_t = 7)]
    pub retain_hourly_days: u64,
    #[arg(help = "How often to thin workspace history (0: never; `ws prune` still works)")]
    #[arg(long, default_value_t = 600)]
    pub retain_interval_secs: u64,
    #[arg(help = "Allow workspaces below this directory instead of below the caller's home (for tests)")]
    #[arg(long)]
    pub workspace_root: Option<PathBuf>,
}

pub struct Paths {
    pub state: PathBuf,
    pub run: PathBuf,
}

impl Paths {
    pub fn db(&self) -> PathBuf {
        self.state.join("meta.db")
    }
    pub fn base(&self, id: &str) -> PathBuf {
        self.state.join("bases").join(id)
    }
    pub fn branch(&self, id: &str) -> PathBuf {
        self.state.join("branches").join(id)
    }
    pub fn snap(&self, id: &str) -> PathBuf {
        self.state.join("snaps").join(id)
    }
    pub fn tmp(&self) -> PathBuf {
        self.state.join("tmp")
    }
    pub fn mnt(&self, branch: &str) -> PathBuf {
        self.run.join("mnt").join(branch)
    }
    pub fn resolve(&self, id: &str) -> PathBuf {
        self.run.join("resolve").join(id)
    }
    pub fn views(&self) -> PathBuf {
        self.run.join("views")
    }
}

pub struct Daemon {
    pub cfg: Config,
    pub paths: Paths,
    pub locks: Locks,
    pub watch: Watcher,
    pub callers: Callers,
    pub launches: Mutex<std::collections::HashMap<String, Launch>>,
    pub _db: Mutex<Connection>,
}

impl Daemon {
    pub fn db(&self) -> Result<Connection> {
        open_db(&self.paths.db())
    }

    pub fn check_ws_path(&self, uid: u32, p: &Path) -> Result<()> {
        let root = match &self.cfg.workspace_root {
            Some(r) => r.clone(),
            None => {
                let passwd = std::fs::read_to_string("/etc/passwd")?;
                PathBuf::from(home_of(&passwd, uid).ok_or_else(|| invalid(format!("uid {uid} has no home directory")))?)
            }
        };
        let root = std::fs::canonicalize(&root).map_err(|e| invalid(format!("home directory {}: {e}", root.display())))?;
        if root == Path::new("/") || p == root || !p.starts_with(&root) {
            return Err(invalid(format!("workspace path {} is not below {}", p.display(), root.display())));
        }
        Ok(())
    }
}

fn home_of(passwd: &str, uid: u32) -> Option<&str> {
    passwd.lines().map(|l| l.split(':').collect::<Vec<_>>()).find(|f| f.len() > 5 && f[2].parse() == Ok(uid)).map(|f| f[5]).filter(|h| !h.is_empty())
}

#[derive(Debug)]
pub struct ApiError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ApiError {}

pub fn api(code: &'static str, message: impl Into<String>) -> anyhow::Error {
    ApiError { code, message: message.into() }.into()
}

pub fn not_found(what: &str, id: &str) -> anyhow::Error {
    api("not_found", format!("{what} {id:?} not found"))
}

pub fn invalid(message: impl Into<String>) -> anyhow::Error {
    api("invalid", message)
}

pub fn bad_state(message: impl Into<String>) -> anyhow::Error {
    api("state", message)
}

pub fn new_id() -> String {
    ulid::Ulid::generate().to_string().to_lowercase()
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS workspaces(
  id TEXT PRIMARY KEY, name TEXT UNIQUE NOT NULL, host_path TEXT NOT NULL,
  uid INTEGER NOT NULL, gid INTEGER NOT NULL, main_branch TEXT NOT NULL, created_at INTEGER NOT NULL,
  live INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS bases(
  id TEXT PRIMARY KEY, ws TEXT NOT NULL, layers INTEGER NOT NULL, from_snap TEXT, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS sessions(
  id TEXT PRIMARY KEY, name TEXT, state TEXT NOT NULL, created_at INTEGER NOT NULL,
  idle_timeout_ms INTEGER, owner_uid INTEGER, caller_pid INTEGER, caller_start INTEGER);
CREATE TABLE IF NOT EXISTS session_live(
  session TEXT NOT NULL, ws TEXT NOT NULL, start_snap TEXT NOT NULL, end_snap TEXT, PRIMARY KEY(session, ws));
CREATE TABLE IF NOT EXISTS procs(
  id TEXT PRIMARY KEY, state TEXT NOT NULL, cmd TEXT NOT NULL, cwd TEXT NOT NULL,
  owner_uid INTEGER NOT NULL, owner_gid INTEGER NOT NULL, pid INTEGER, pid_start INTEGER, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS proc_branches(
  proc TEXT NOT NULL, ws TEXT NOT NULL, branch TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'open',
  PRIMARY KEY(proc, ws));
CREATE TABLE IF NOT EXISTS branches(
  id TEXT PRIMARY KEY, ws TEXT NOT NULL, base TEXT NOT NULL, kind TEXT NOT NULL, owner TEXT NOT NULL,
  parent_branch TEXT, head_snap TEXT NOT NULL, fork_snap TEXT, alive INTEGER NOT NULL DEFAULT 1);
CREATE TABLE IF NOT EXISTS changesets(
  id TEXT PRIMARY KEY, owner_kind TEXT NOT NULL, owner TEXT NOT NULL, author TEXT NOT NULL,
  message TEXT NOT NULL, tool_call TEXT, created_at INTEGER NOT NULL, session TEXT);
CREATE TABLE IF NOT EXISTS snaps(
  id TEXT PRIMARY KEY, branch TEXT NOT NULL, ws TEXT NOT NULL, changeset TEXT NOT NULL,
  parent TEXT, merge_parent TEXT, base TEXT NOT NULL, created_at INTEGER NOT NULL,
  present INTEGER NOT NULL DEFAULT 1, summary TEXT, session TEXT);
CREATE INDEX IF NOT EXISTS snaps_changeset ON snaps(changeset);
CREATE INDEX IF NOT EXISTS snaps_branch ON snaps(branch);
CREATE TABLE IF NOT EXISTS journal(
  branch TEXT PRIMARY KEY, restore_snap TEXT NOT NULL, op TEXT NOT NULL, created_at INTEGER NOT NULL);
"#;

pub fn open_db(p: &Path) -> Result<Connection> {
    let c = Connection::open(p).with_context(|| format!("open {}", p.display()))?;
    c.busy_timeout(std::time::Duration::from_secs(30))?;
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=OFF;")?;
    Ok(c)
}

const INDEXES: &str = "
CREATE INDEX IF NOT EXISTS snaps_ws ON snaps(ws, changeset);
CREATE INDEX IF NOT EXISTS changesets_session ON changesets(session);
CREATE INDEX IF NOT EXISTS changesets_owner ON changesets(owner, owner_kind);
CREATE INDEX IF NOT EXISTS sessions_name ON sessions(name);
CREATE INDEX IF NOT EXISTS session_live_ws ON session_live(ws);
CREATE INDEX IF NOT EXISTS proc_branches_branch ON proc_branches(branch);
CREATE INDEX IF NOT EXISTS snaps_session ON snaps(session);
";

const RETIRED: &str = "
UPDATE branches SET alive = 0 WHERE alive = 1 AND kind IN ('session', 'process') AND id NOT IN (SELECT branch FROM proc_branches);
DROP TABLE IF EXISTS session_branches;
DROP TABLE IF EXISTS proc_cwds;
DROP TABLE IF EXISTS processes;
";

pub fn init_db(p: &Path) -> Result<Connection> {
    let c = open_db(p)?;
    c.execute_batch(SCHEMA)?;
    if c.query_row("SELECT COUNT(*) FROM pragma_table_info('snaps') WHERE name = 'session'", [], |r| r.get::<_, i64>(0))? == 0 {
        c.execute_batch("ALTER TABLE snaps ADD COLUMN session TEXT")?;
    }
    c.execute_batch(INDEXES)?;
    c.execute_batch(RETIRED)?;
    Ok(c)
}

pub fn rows<T, C: FromIterator<T>>(c: &Connection, sql: &str, params: impl Params, f: impl FnMut(&Row) -> rusqlite::Result<T>) -> Result<C> {
    Ok(c.prepare(sql)?.query_map(params, f)?.collect::<rusqlite::Result<C>>()?)
}

pub fn col<T: FromSql, C: FromIterator<T>>(c: &Connection, sql: &str, params: impl Params) -> Result<C> {
    rows(c, sql, params, |r| r.get(0))
}

#[derive(Debug, Clone)]
pub struct Ws {
    pub id: String,
    pub name: String,
    pub host_path: String,
    pub uid: u32,
    pub gid: u32,
    pub main_branch: String,
    pub live: bool,
}

impl Ws {
    pub fn keys(&self) -> [String; 2] {
        [format!("ws:{}", self.id), k_branch(&self.main_branch)]
    }
}

const WS_COLS: &str = "id, name, host_path, uid, gid, main_branch, live";

fn ws_row(r: &Row) -> rusqlite::Result<Ws> {
    Ok(Ws { id: r.get(0)?, name: r.get(1)?, host_path: r.get(2)?, uid: r.get(3)?, gid: r.get(4)?, main_branch: r.get(5)?, live: r.get::<_, i64>(6)? != 0 })
}

pub fn ws_get(c: &Connection, key: &str) -> Result<Ws> {
    c.query_row(&format!("SELECT {WS_COLS} FROM workspaces WHERE id = ?1 OR name = ?1"), [key], ws_row).optional()?.ok_or_else(|| not_found("workspace", key))
}

pub fn live_wss(c: &Connection) -> Result<Vec<Ws>> {
    rows(c, &format!("SELECT {WS_COLS} FROM workspaces WHERE live = 1 ORDER BY name"), [], ws_row)
}

pub fn live_ws_of_branch(c: &Connection, branch: &str) -> Result<Option<Ws>> {
    Ok(c.query_row(&format!("SELECT {WS_COLS} FROM workspaces WHERE main_branch = ?1 AND live = 1"), [branch], ws_row).optional()?)
}

pub fn ws_session(c: &Connection, ws: &str) -> Result<Option<String>> {
    let sql = "SELECT l.session FROM session_live l JOIN sessions s ON s.id = l.session WHERE l.ws = ?1 AND l.end_snap IS NULL AND s.state = 'active'";
    Ok(c.query_row(sql, [ws], |r| r.get(0)).optional()?)
}

#[derive(Debug, Clone)]
pub struct Branch {
    pub id: String,
    pub ws: String,
    pub base: String,
    pub head_snap: String,
}

pub fn branch_get(c: &Connection, id: &str) -> Result<Branch> {
    c.query_row("SELECT id, ws, base, head_snap FROM branches WHERE id = ?1", [id], |r| {
        Ok(Branch { id: r.get(0)?, ws: r.get(1)?, base: r.get(2)?, head_snap: r.get(3)? })
    })
    .optional()?
    .ok_or_else(|| not_found("branch", id))
}

#[derive(Debug, Clone)]
pub struct Snap {
    pub id: String,
    pub branch: String,
    pub ws: String,
    pub parent: Option<String>,
    pub merge_parent: Option<String>,
    pub base: String,
    pub present: bool,
    pub summary: Option<String>,
}

fn snap_row(r: &rusqlite::Row) -> rusqlite::Result<Snap> {
    Ok(Snap {
        id: r.get(0)?,
        branch: r.get(1)?,
        ws: r.get(2)?,
        parent: r.get(3)?,
        merge_parent: r.get(4)?,
        base: r.get(5)?,
        present: r.get::<_, i64>(6)? != 0,
        summary: r.get(7)?,
    })
}

const SNAP_COLS: &str = "id, branch, ws, parent, merge_parent, base, present, summary";

pub fn snap_get(c: &Connection, id: &str) -> Result<Snap> {
    c.query_row(&format!("SELECT {SNAP_COLS} FROM snaps WHERE id = ?1"), [id], snap_row).optional()?.ok_or_else(|| not_found("snapshot", id))
}

pub fn snaps_of_changeset(c: &Connection, cs: &str) -> Result<Vec<Snap>> {
    rows(c, &format!("SELECT {SNAP_COLS} FROM snaps WHERE changeset = ?1 ORDER BY ws"), [cs], snap_row)
}

pub fn snap_parents(c: &Connection, id: &str) -> Vec<String> {
    c.query_row("SELECT parent, merge_parent FROM snaps WHERE id = ?1", [id], |r| Ok([r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?]))
        .map(|ps| ps.into_iter().flatten().collect())
        .unwrap_or_default()
}

pub fn snap_present(c: &Connection, id: &str) -> bool {
    c.query_row("SELECT present FROM snaps WHERE id = ?1", [id], |r| r.get::<_, i64>(0)).map(|v| v != 0).unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub state: String,
    pub owner_uid: u32,
}

impl Session {
    pub fn ensure_active(&self) -> Result<()> {
        if self.state != "active" {
            return Err(bad_state(format!("session is {}", self.state)));
        }
        Ok(())
    }
}

pub fn session_get(c: &Connection, id: &str) -> Result<Session> {
    c.query_row("SELECT id, state, IFNULL(owner_uid, 0) FROM sessions WHERE id = ?1 OR name = ?1", [id], |r| {
        Ok(Session { id: r.get(0)?, state: r.get(1)?, owner_uid: r.get(2)? })
    })
    .optional()?
    .ok_or_else(|| not_found("session", id))
}

pub const ROOT: u32 = 0;

pub fn ws_for(c: &Connection, uid: u32, key: &str) -> Result<Ws> {
    ws_get(c, key).ok().filter(|w| uid == ROOT || w.uid == uid).ok_or_else(|| not_found("workspace", key))
}

pub fn session_for(c: &Connection, uid: u32, key: &str) -> Result<Session> {
    session_get(c, key).ok().filter(|s| uid == ROOT || s.owner_uid == uid).ok_or_else(|| not_found("session", key))
}

pub fn check_snaps(c: &Connection, uid: u32, key: &str, snaps: &[Snap]) -> Result<()> {
    if snaps.iter().all(|s| ws_for(c, uid, &s.ws).is_ok()) { Ok(()) } else { Err(not_found("changeset or snapshot", key)) }
}

#[derive(Clone)]
pub struct NewSnap {
    pub id: String,
    pub branch: String,
    pub ws: String,
    pub parent: Option<String>,
    pub merge_parent: Option<String>,
    pub base: String,
}

impl NewSnap {
    pub fn after(b: &Branch, id: String, merge_parent: Option<String>) -> NewSnap {
        NewSnap { id, branch: b.id.clone(), ws: b.ws.clone(), parent: Some(b.head_snap.clone()), merge_parent, base: b.base.clone() }
    }
}

pub struct NewChangeset<'a> {
    pub id: String,
    pub owner_kind: &'a str,
    pub owner: &'a str,
    pub author: &'a str,
    pub message: &'a str,
    pub tool_call: Option<&'a str>,
    pub session: Option<&'a str>,
}

impl<'a> NewChangeset<'a> {
    pub fn new(id: String, owner_kind: &'a str, owner: &'a str, author: &'a str, message: &'a str) -> Self {
        NewChangeset { id, owner_kind, owner, author, message, tool_call: None, session: None }
    }
}

pub fn record_changeset(c: &mut Connection, cs: &NewChangeset, snaps: &[NewSnap]) -> Result<()> {
    let t = c.transaction()?;
    let now = now_ms();
    t.execute(
        "INSERT INTO changesets(id, owner_kind, owner, author, message, tool_call, created_at, session) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![cs.id, cs.owner_kind, cs.owner, cs.author, cs.message, cs.tool_call, now, cs.session],
    )?;
    for s in snaps {
        let session = ws_session(&t, &s.ws)?;
        t.execute(
            "INSERT INTO snaps(id, branch, ws, changeset, parent, merge_parent, base, created_at, session) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![s.id, s.branch, s.ws, cs.id, s.parent, s.merge_parent, s.base, now, session],
        )?;
        t.execute("UPDATE branches SET head_snap = ?1, base = ?2 WHERE id = ?3", params![s.id, s.base, s.branch])?;
    }
    t.commit()?;
    Ok(())
}

pub fn journal_begin(c: &Connection, branch: &str, restore_snap: &str, op: &str) -> Result<()> {
    c.execute("INSERT OR REPLACE INTO journal(branch, restore_snap, op, created_at) VALUES (?1,?2,?3,?4)", params![branch, restore_snap, op, now_ms()])?;
    Ok(())
}

pub fn journal_end(c: &Connection, branch: &str) -> Result<()> {
    c.execute("DELETE FROM journal WHERE branch = ?1", [branch])?;
    Ok(())
}

#[derive(Default)]
pub struct Locks {
    held: Mutex<HashSet<String>>,
    cv: Condvar,
}

pub struct Guard<'a> {
    locks: &'a Locks,
    keys: Vec<String>,
}

impl Locks {
    pub fn lock<I: IntoIterator<Item = String>>(&self, keys: I) -> Guard<'_> {
        let mut keys: Vec<String> = keys.into_iter().collect();
        keys.sort();
        keys.dedup();
        let mut held = self.held.lock().unwrap();
        while keys.iter().any(|k| held.contains(k)) {
            held = self.cv.wait(held).unwrap();
        }
        for k in &keys {
            held.insert(k.clone());
        }
        Guard { locks: self, keys }
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let mut held = self.locks.held.lock().unwrap();
        for k in &self.keys {
            held.remove(k);
        }
        self.locks.cv.notify_all();
    }
}

pub fn k_branch(id: &str) -> String {
    format!("branch:{id}")
}
pub fn k_proc(id: &str) -> String {
    format!("proc:{id}")
}
pub fn k_session(id: &str) -> String {
    format!("session:{id}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn home_of() {
        let p = "root:x:0:0::/root:/bin/bash\nnobody:x:65534:65534::/:/sbin/nologin\nnohome:x:7:7:::\na:x:1000:1000:A,,:/home/a:/bin/sh\n";
        assert_eq!(super::home_of(p, 0), Some("/root"));
        assert_eq!(super::home_of(p, 1000), Some("/home/a"));
        assert_eq!(super::home_of(p, 7), None);
        assert_eq!(super::home_of(p, 42), None);
    }
}
