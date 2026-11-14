use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use acfs_core::proto::Remote;
use acfs_core::results::{PullStatus, Pulled, Pushed};
use anyhow::{Result, anyhow, bail};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::daemon::*;
use crate::store::Undo;
use crate::sys;

#[derive(Serialize, Deserialize, Default)]
struct Manifest {
    name: String,
    host_path: String,
    uid: u32,
    gid: u32,
    bases: Vec<Item>,
    snaps: Vec<Item>,
    head: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct Item {
    id: String,
    send_parent: Option<String>,
    sha256: String,
    #[serde(default)]
    layers: u32,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    base: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    author: String,
}

fn remote_dir(d: &Daemon, uid: u32, a: &Remote, ws_name: &str) -> Result<PathBuf> {
    let root = match &a.remote {
        Some(_) if uid != ROOT => return Err(invalid("only root may choose a remote directory; --remote-dir is used otherwise")),
        Some(r) => PathBuf::from(r),
        None => d.cfg.remote_dir.clone().ok_or_else(|| invalid("no remote given and --remote-dir not configured"))?,
    };
    Ok(root.join(ws_name))
}

fn sha_file(p: &Path) -> Result<String> {
    Ok(crate::diff::sha256(fs::File::open(p)?)?.iter().map(|b| format!("{b:02x}")).collect())
}

fn send(dir: &Path, id: &str, subvol: &Path, parent: Option<&Path>) -> Result<(String, u64)> {
    let file = dir.join("streams").join(format!("{id}.btrfs"));
    let mut cmd = Command::new("btrfs");
    cmd.args(["send", "-q", "-f"]).arg(&file);
    if let Some(p) = parent {
        cmd.arg("-p").arg(p);
    }
    sys::run(cmd.arg(subvol))?;
    Ok((sha_file(&file)?, fs::metadata(&file)?.len()))
}

fn load(dir: &Path) -> Result<Option<Manifest>> {
    let p = dir.join("manifest.json");
    if !p.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&fs::read(p)?)?))
}

pub fn push(d: &Daemon, uid: u32, a: Remote) -> Result<Pushed> {
    let mut c = d.db()?;
    let ws = ws_for(&c, uid, &a.ws)?;
    let _g = d.locks.lock(ws.keys());
    if ws.live {
        super::live::sync(d, &mut c, &ws, "before push")?;
    }
    let dir = remote_dir(d, uid, &a, &ws.name)?;
    fs::create_dir_all(dir.join("streams"))?;
    let mut m = load(&dir)?.unwrap_or_default();
    if !m.name.is_empty() && m.name != ws.name {
        bail!("remote holds workspace {:?}", m.name);
    }
    if !m.name.is_empty() && uid != ROOT && m.uid != uid {
        return Err(invalid(format!("remote workspace {} belongs to uid {}", m.name, m.uid)));
    }
    m.name = ws.name.clone();
    m.host_path = ws.host_path.clone();
    m.uid = ws.uid;
    m.gid = ws.gid;
    let main = branch_get(&c, &ws.main_branch)?;
    let sql = "SELECT s.id, s.parent, s.base, cs.message, cs.author FROM snaps s JOIN changesets cs ON cs.id = s.changeset
               WHERE s.branch = ?1 AND s.present = 1 ORDER BY s.created_at, s.id";
    let snaps: Vec<(String, Option<String>, String, String, String)> =
        rows(&c, sql, [&main.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
    let (mut nb, mut ns, mut bytes) = (0, 0, 0u64);
    let have = |m: &Manifest, id: &str| m.bases.iter().chain(&m.snaps).any(|i| i.id == id);
    for (sid, parent, base, message, author) in &snaps {
        if !have(&m, base) {
            let (layers, from): (u32, Option<String>) =
                c.query_row("SELECT layers, from_snap FROM bases WHERE id = ?1", [base], |r| Ok((r.get(0)?, r.get(1)?)))?;
            let sp = from.filter(|f| have(&m, f));
            let (sha256, n) = send(&dir, base, &d.paths.base(base), sp.as_ref().map(|p| d.paths.snap(p)).as_deref())?;
            bytes += n;
            m.bases.push(Item { id: base.clone(), send_parent: sp, sha256, layers, ..Default::default() });
            nb += 1;
        }
        if !have(&m, sid) {
            let sp = parent.clone().filter(|p| have(&m, p)).unwrap_or_else(|| base.clone());
            let sp_path = if m.bases.iter().any(|b| b.id == sp) { d.paths.base(&sp) } else { d.paths.snap(&sp) };
            let (sha256, n) = send(&dir, sid, &d.paths.snap(sid), Some(&sp_path))?;
            bytes += n;
            m.snaps.push(Item {
                id: sid.clone(),
                send_parent: Some(sp),
                sha256,
                parent: parent.clone().filter(|p| have(&m, p)),
                base: base.clone(),
                message: message.clone(),
                author: author.clone(),
                ..Default::default()
            });
            ns += 1;
        }
    }
    m.head = main.head_snap.clone();
    let tmp = dir.join("manifest.json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&m)?)?;
    fs::rename(&tmp, dir.join("manifest.json"))?;
    Ok(Pushed { ws: ws.name, remote: dir.to_string_lossy().into_owned(), bases_sent: nb, snaps_sent: ns, bytes, head: m.head })
}

fn receive(d: &Daemon, dir: &Path, item: &Item, into: &Path) -> Result<()> {
    if into.join(&item.id).exists() {
        return Ok(());
    }
    let file = dir.join("streams").join(format!("{}.btrfs", item.id));
    let sum = sha_file(&file)?;
    if sum != item.sha256 {
        bail!("checksum mismatch for {}: {} != {}", item.id, sum, item.sha256);
    }
    if let Err(e) = sys::run(Command::new("btrfs").args(["receive", "-q", "-f"]).arg(&file).arg(into)) {
        let _ = d.delete_subvol(&into.join(&item.id));
        return Err(e);
    }
    Ok(())
}

pub fn pull(d: &Daemon, uid: u32, a: Remote) -> Result<Pulled> {
    let dir = remote_dir(d, uid, &a, &a.ws)?;
    let m = load(&dir)?.ok_or_else(|| not_found("remote workspace", &a.ws))?;
    if uid != ROOT && m.uid != uid {
        return Err(not_found("remote workspace", &a.ws));
    }
    d.check_ws_path(uid, Path::new(&m.host_path))?;
    let _g = d.locks.lock([format!("wsname:{}", m.name)]);
    let mut c = d.db()?;
    if ws_get(&c, &m.name).is_ok() && ws_for(&c, uid, &m.name).is_err() {
        return Err(api("exists", format!("workspace {:?} already exists", m.name)));
    }
    let local = ws_get(&c, &m.name).ok();
    if let Some(ws) = &local {
        if ws.live {
            return Err(bad_state(format!("{} is live; detach it before pulling", ws.name)));
        }
        let main = branch_get(&c, &ws.main_branch)?;
        if main.head_snap == m.head {
            return Ok(Pulled { ws: ws.name.clone(), status: PullStatus::UpToDate, bases_received: 0, snaps_received: 0, head: m.head });
        }
        if !m.snaps.iter().any(|s| s.id == main.head_snap) {
            return Err(bad_state("local main has commits the remote lacks; push first"));
        }
    }
    let (mut nb, mut ns) = (0, 0);
    let mut undo = Undo::new(d);
    let have = |id: &str| d.paths.base(id).exists() || d.paths.snap(id).exists();
    let mut pending: Vec<(&Item, bool)> = m.bases.iter().map(|b| (b, true)).chain(m.snaps.iter().map(|s| (s, false))).filter(|(i, _)| !have(&i.id)).collect();
    while !pending.is_empty() {
        let next = pending
            .iter()
            .position(|(i, _)| i.send_parent.as_deref().is_none_or(have))
            .ok_or_else(|| anyhow!("remote manifest: send parent missing for {}", pending[0].0.id))?;
        let (item, is_base) = pending.remove(next);
        let (into, path) = if is_base { ("bases", d.paths.base(&item.id)) } else { ("snaps", d.paths.snap(&item.id)) };
        receive(d, &dir, item, &d.paths.state.join(into))?;
        undo.subvol(path);
        if is_base {
            nb += 1;
        } else {
            ns += 1;
        }
    }
    undo.commit();
    let ws_id = local.as_ref().map(|w| w.id.clone()).unwrap_or_else(new_id);
    let main_id = local.as_ref().map(|w| w.main_branch.clone()).unwrap_or_else(new_id);
    let head = m.snaps.iter().find(|s| s.id == m.head).ok_or_else(|| anyhow!("manifest head missing"))?;
    let t = c.transaction()?;
    let now = now_ms();
    if local.is_none() {
        t.execute(
            "INSERT INTO workspaces(id, name, host_path, uid, gid, main_branch, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![ws_id, m.name, m.host_path, m.uid, m.gid, main_id, now],
        )?;
    }
    for b in &m.bases {
        t.execute("INSERT OR IGNORE INTO bases(id, ws, layers, created_at) VALUES (?1,?2,?3,?4)", params![b.id, ws_id, b.layers, now])?;
    }
    for s in &m.snaps {
        let exists: bool = t.query_row("SELECT 1 FROM snaps WHERE id = ?1", [&s.id], |_| Ok(true)).unwrap_or(false);
        if exists {
            continue;
        }
        let cs = new_id();
        t.execute(
            "INSERT INTO changesets(id, owner_kind, owner, author, message, created_at) VALUES (?1,'workspace',?2,?3,?4,?5)",
            params![cs, ws_id, s.author, format!("{} (pulled)", s.message), now],
        )?;
        t.execute(
            "INSERT INTO snaps(id, branch, ws, changeset, parent, base, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![s.id, main_id, ws_id, cs, s.parent, s.base, now],
        )?;
    }
    if local.is_none() {
        t.execute("INSERT INTO branches(id, ws, base, kind, owner, head_snap) VALUES (?1,?2,?3,'main',?2,?4)", params![main_id, ws_id, head.base, m.head])?;
    } else {
        t.execute("UPDATE branches SET head_snap = ?1, base = ?2 WHERE id = ?3", params![m.head, head.base, main_id])?;
    }
    d.reset_branch(&main_id, &m.head)?;
    t.commit()?;
    let status = if local.is_some() { PullStatus::FastForward } else { PullStatus::Created };
    Ok(Pulled { ws: m.name, status, bases_received: nb, snaps_received: ns, head: m.head })
}
