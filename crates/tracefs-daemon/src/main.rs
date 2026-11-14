mod callers;
mod daemon;
mod diff;
mod helper;
mod merging;
mod ops;
mod recover;
mod send;
mod server;
mod store;
mod sys;
mod tree;
mod watch;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Parser;
use tracefs_core::names;

use daemon::{Config, Daemon, Locks, Paths};

pub fn failpoint(name: &str) {
    if std::env::var("TRACEFS_FAILPOINT").is_ok_and(|v| v == name) {
        eprintln!("tracefs-daemon: failpoint {name}");
        std::process::abort();
    }
}

fn prepare_state(dir: &Path, size: &str) -> Result<()> {
    fs::create_dir_all(dir)?;
    if sys::is_btrfs(dir)? {
        return Ok(());
    }
    let img = PathBuf::from(format!("{}.img", dir.display()));
    if !img.exists() {
        eprintln!("tracefs-daemon: {} is not on btrfs; creating {}", dir.display(), img.display());
        fs::File::create(&img)?;
        let _ = Command::new("chattr").arg("+C").arg(&img).status();
        sys::run(Command::new("truncate").args(["-s", size]).arg(&img))?;
        sys::run(Command::new("mkfs.btrfs").args(["-q", "-f"]).arg(&img))?;
    }
    let out = sys::run(Command::new("losetup").args(["-f", "--show", "--direct-io=on", "--sector-size", "4096"]).arg(&img))?;
    let dev = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mounted = sys::run(Command::new("mount").args(["-o", "noatime"]).arg(&dev).arg(dir));
    let _ = Command::new("losetup").args(["-d", &dev]).status();
    mounted?;
    if !sys::is_btrfs(dir)? {
        bail!("could not mount {} on {}", img.display(), dir.display());
    }
    Ok(())
}

fn run(cfg: Config) -> Result<()> {
    if !rustix::process::geteuid().is_root() {
        bail!("tracefs-daemon must run as root");
    }
    if !names::valid_slice(&cfg.slice) {
        bail!("invalid slice name {:?} (lowercase letters and digits)", cfg.slice);
    }
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o022));
    let paths = Paths { state: cfg.state_dir.clone(), run: cfg.run_dir.clone() };
    prepare_state(&paths.state, &cfg.image_size)?;
    fs::set_permissions(&paths.state, fs::Permissions::from_mode(0o700))?;
    for d in ["bases", "branches", "snaps", "tmp"] {
        fs::create_dir_all(paths.state.join(d))?;
    }
    if !paths.db().exists() && fs::read_dir(paths.state.join("bases"))?.next().is_some() {
        bail!("{} is missing but {} holds data; refusing to start", paths.db().display(), paths.state.display());
    }
    fs::create_dir_all(&paths.run)?;
    let boot = !sys::is_mountpoint(&paths.run);
    if boot {
        sys::mount_private_tmpfs(&paths.run)?;
    }
    for d in ["mnt", "views", "resolve"] {
        fs::create_dir_all(paths.run.join(d))?;
        fs::set_permissions(paths.run.join(d), fs::Permissions::from_mode(0o711))?;
    }
    let db = std::sync::Mutex::new(daemon::init_db(&paths.db())?);
    let watch = watch::Watcher::new().context("inotify")?;
    let d = Arc::new(Daemon { cfg, paths, locks: Locks::default(), watch, callers: Default::default(), launches: Default::default(), _db: db });
    recover::run(&d)?;
    ops::procs::recover(&d)?;
    if boot {
        ops::session::end_after_boot(&d)?;
    }
    d.callers.load(&d.db()?)?;
    let w = d.clone();
    std::thread::spawn(move || watch::run(w));
    server::serve(d)
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some("ns-helper") {
        if let Err(e) = helper::main(&argv[2..]) {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(e) = run(Config::parse()) {
        eprintln!("tracefs-daemon: {e:#}");
        std::process::exit(1);
    }
}
