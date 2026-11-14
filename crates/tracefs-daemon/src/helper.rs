use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use rustix::mount::{MountFlags, MountPropagationFlags};
use rustix::process::{Gid, Uid};
use serde::{Deserialize, Serialize};

pub const SPEC_ENV: &str = "TRACEFS_RUN_SPEC";

#[derive(Serialize, Deserialize)]
pub struct RunSpec {
    pub binds: Vec<(String, String)>,
    pub hide: Vec<String>,
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
    pub cwd: String,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
}

pub fn main(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("run") => {
            let spec = std::env::var(SPEC_ENV).context("no run spec")?;
            let spec: RunSpec = serde_json::from_str(&spec)?;
            let e = run(spec);
            eprintln!("tracefs: {e:#}");
            std::process::exit(127);
        }
        _ => bail!("usage: tracefsd ns-helper run"),
    }
}

fn run(spec: RunSpec) -> anyhow::Error {
    match enter(&spec) {
        Ok(()) => {
            let mut cmd = Command::new(&spec.cmd[0]);
            cmd.args(&spec.cmd[1..]).env_clear().envs(spec.env.iter().filter_map(|e| e.split_once('=')));
            anyhow::Error::from(cmd.exec()).context(format!("exec {}", spec.cmd[0]))
        }
        Err(e) => e,
    }
}

fn enter(spec: &RunSpec) -> Result<()> {
    unsafe { rustix::thread::unshare_unsafe(rustix::thread::UnshareFlags::NEWNS) }.context("unshare(CLONE_NEWNS)")?;
    rustix::mount::mount_change("/", MountPropagationFlags::DOWNSTREAM | MountPropagationFlags::REC).context("make / a slave")?;
    for (src, dst) in &spec.binds {
        rustix::mount::mount_bind(src.as_str(), dst.as_str()).with_context(|| format!("bind {src} on {dst}"))?;
    }
    for p in &spec.hide {
        if Path::new(p).is_dir() {
            rustix::mount::mount("tmpfs", p.as_str(), "tmpfs", MountFlags::NOSUID | MountFlags::NODEV | MountFlags::NOEXEC, c"size=4k,mode=0711")
                .with_context(|| format!("hide {p}"))?;
        }
    }
    let groups: Vec<Gid> = spec.groups.iter().map(|g| Gid::from_raw(*g)).collect();
    rustix::thread::set_thread_groups(&groups).context("setgroups")?;
    let (gid, uid) = (Gid::from_raw(spec.gid), Uid::from_raw(spec.uid));
    rustix::thread::set_thread_res_gid(gid, gid, gid).context("setresgid")?;
    rustix::thread::set_thread_res_uid(uid, uid, uid).context("setresuid")?;
    rustix::process::setsid().context("setsid")?;
    rustix::process::chdir(spec.cwd.as_str()).with_context(|| format!("cd {}", spec.cwd))?;
    Ok(())
}
