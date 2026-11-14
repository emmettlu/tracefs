use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};
use rustix::io::Errno;

use crate::daemon::Daemon;
use crate::ops;

#[derive(Default)]
struct State {
    wds: HashMap<i32, (String, PathBuf)>,
    dirty: HashMap<String, (Instant, Instant)>,
}

pub struct Watcher {
    fd: OwnedFd,
    state: Mutex<State>,
}

fn mask() -> WatchFlags {
    WatchFlags::MODIFY
        | WatchFlags::ATTRIB
        | WatchFlags::CLOSE_WRITE
        | WatchFlags::CREATE
        | WatchFlags::DELETE
        | WatchFlags::MOVED_FROM
        | WatchFlags::MOVED_TO
        | WatchFlags::ONLYDIR
}

fn mark(st: &mut State, ws: String, now: Instant) {
    st.dirty.entry(ws).and_modify(|e| e.1 = now).or_insert((now, now));
}

fn mark_all(st: &mut State, now: Instant) {
    let all: Vec<String> = st.wds.values().map(|(w, _)| w.clone()).collect();
    all.into_iter().for_each(|w| mark(st, w, now));
}

impl Watcher {
    pub fn new() -> io::Result<Watcher> {
        Ok(Watcher { fd: inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?, state: Mutex::default() })
    }

    pub fn add(&self, ws: &str, upper: &Path) {
        self.add_tree(&mut self.state.lock().unwrap(), ws, upper);
    }

    fn add_tree(&self, st: &mut State, ws: &str, dir: &Path) {
        match inotify::add_watch(&self.fd, dir, mask()) {
            Ok(wd) => st.wds.insert(wd, (ws.to_string(), dir.to_path_buf())),
            Err(Errno::NOENT | Errno::NOTDIR) => return,
            Err(e) => {
                eprintln!("acfs-daemon: watch {}: {e}", dir.display());
                return;
            }
        };
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                self.add_tree(st, ws, &e.path());
            }
        }
    }

    pub fn remove(&self, ws: &str) {
        let mut st = self.state.lock().unwrap();
        let wds: Vec<i32> = st.wds.iter().filter(|(_, (w, _))| w == ws).map(|(wd, _)| *wd).collect();
        for wd in wds {
            let _ = inotify::remove_watch(&self.fd, wd);
            st.wds.remove(&wd);
        }
        st.dirty.remove(ws);
    }

    fn drain(&self) {
        let mut buf = vec![MaybeUninit::<u8>::uninit(); 64 << 10];
        let mut r = inotify::Reader::new(&self.fd, &mut buf);
        let mut st = self.state.lock().unwrap();
        let now = Instant::now();
        loop {
            let ev = match r.next() {
                Ok(ev) => ev,
                Err(Errno::AGAIN) => break,
                Err(e) => {
                    eprintln!("acfs-daemon: read inotify: {e}");
                    break;
                }
            };
            let flags = ev.events();
            if flags.contains(ReadFlags::QUEUE_OVERFLOW) {
                mark_all(&mut st, now);
                continue;
            }
            if flags.contains(ReadFlags::IGNORED) {
                st.wds.remove(&ev.wd());
                continue;
            }
            let Some((ws, dir)) = st.wds.get(&ev.wd()).cloned() else { continue };
            mark(&mut st, ws.clone(), now);
            if flags.contains(ReadFlags::ISDIR)
                && flags.intersects(ReadFlags::CREATE | ReadFlags::MOVED_TO)
                && let Some(n) = ev.file_name()
            {
                self.add_tree(&mut st, &ws, &dir.join(OsStr::from_bytes(n.to_bytes())));
            }
        }
    }

    fn due(&self, quiet: Duration, max: Duration) -> Vec<String> {
        let now = Instant::now();
        let mut st = self.state.lock().unwrap();
        let due: Vec<String> = st.dirty.iter().filter(|(_, (first, last))| now - *last >= quiet || now - *first >= max).map(|(w, _)| w.clone()).collect();
        for w in &due {
            st.dirty.remove(w);
        }
        due
    }

    pub fn touch(&self, ws: &str) {
        mark(&mut self.state.lock().unwrap(), ws.to_string(), Instant::now());
    }

    pub fn is_dirty(&self, ws: &str) -> bool {
        self.state.lock().unwrap().dirty.contains_key(ws)
    }
}

pub fn run(d: Arc<Daemon>) {
    let quiet = Duration::from_millis(d.cfg.auto_quiet_ms);
    let max = Duration::from_millis(d.cfg.auto_max_ms);
    let (mut idle_check, mut sweep, mut thinned) = (Instant::now(), Instant::now(), Instant::now());
    let thin_every = Duration::from_secs(d.cfg.retain_interval_secs);
    loop {
        std::thread::sleep(Duration::from_millis(200));
        d.watch.drain();
        if sweep.elapsed() >= max * 10 {
            sweep = Instant::now();
            mark_all(&mut d.watch.state.lock().unwrap(), Instant::now());
        }
        for key in d.watch.due(quiet, max) {
            let r = match ops::procs::branch_of_key(&key) {
                Some(b) => ops::procs::auto_snapshot(&d, b),
                None => ops::live::auto_snapshot(&d, &key),
            };
            if let Err(e) = r {
                eprintln!("acfs-daemon: auto snapshot of {key}: {e:#}");
            }
        }
        if idle_check.elapsed() >= Duration::from_secs(1) {
            idle_check = Instant::now();
            ops::session::end_idle(&d);
        }
        if !thin_every.is_zero() && thinned.elapsed() >= thin_every {
            thinned = Instant::now();
            ops::retention::prune_live(&d);
        }
    }
}
