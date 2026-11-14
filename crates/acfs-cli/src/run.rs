use std::io::{BufRead, BufReader, IoSlice, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;

use acfs_core::proto::{ErrorBody, ProcRun, Request, Response};
use acfs_core::results::{MergeStatus, ProcExited, ProcStarted};
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags};
use serde_json::json;

const SIGNALS: [i32; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("acfs: {msg}");
    std::process::exit(125);
}

fn block_signals() -> libc::sigset_t {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in SIGNALS {
            libc::sigaddset(&mut set, s);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    }
}

fn forward(set: libc::sigset_t, mut s: UnixStream) {
    loop {
        let mut sig: libc::c_int = 0;
        if unsafe { libc::sigwait(&set, &mut sig) } != 0 {
            return;
        }
        let mut line = json!({ "signal": sig }).to_string().into_bytes();
        line.push(b'\n');
        if s.write_all(&line).is_err() {
            return;
        }
    }
}

fn env() -> Vec<String> {
    std::env::vars_os().filter_map(|(k, v)| Some(format!("{}={}", k.to_str()?, v.to_str()?))).collect()
}

fn read(r: &mut BufReader<&UnixStream>) -> Option<Response> {
    let mut line = String::new();
    match r.read_line(&mut line) {
        Ok(n) if n > 0 => serde_json::from_str(&line).ok(),
        _ => None,
    }
}

fn error(r: Response) -> ! {
    let e = r.error.unwrap_or(ErrorBody { code: "internal".into(), message: "unknown error".into() });
    fail(format!("{} ({})", e.message, e.code))
}

pub fn main(socket: &Path, ws: Vec<String>, resolver: Option<String>, cmd: Vec<String>) -> ! {
    let set = block_signals();
    let cwd = std::env::current_dir().unwrap_or_else(|e| fail(format!("current directory: {e}")));
    let cwd = cwd.into_os_string().into_string().unwrap_or_else(|_| fail("the current directory is not UTF-8"));
    let resolver = resolver.map(|r| vec!["/bin/sh".to_string(), "-c".into(), r]).unwrap_or_default();
    let req = Request::ProcRun(ProcRun { ws, cmd, env: env(), cwd, resolver });
    let s = UnixStream::connect(socket).unwrap_or_else(|e| fail(format!("connect {}: {e}", socket.display())));
    let mut line = serde_json::to_vec(&req).unwrap_or_default();
    line.push(b'\n');
    let (stdin, stdout, stderr) = (std::io::stdin(), std::io::stdout(), std::io::stderr());
    let fds = [stdin.as_fd(), stdout.as_fd(), stderr.as_fd()];
    let mut space = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(3))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    control.push(SendAncillaryMessage::ScmRights(&fds));
    let sent = rustix::net::sendmsg(&s, &[IoSlice::new(&line)], &mut control, SendFlags::empty()).unwrap_or_else(|e| fail(format!("send: {e}")));
    (&s).write_all(&line[sent..]).unwrap_or_else(|e| fail(format!("send: {e}")));
    let mut r = BufReader::new(&s);
    let started = read(&mut r).unwrap_or_else(|| fail("the daemon closed the connection"));
    if !started.ok {
        error(started);
    }
    let (id, pid) = serde_json::from_value::<ProcStarted>(started.result).map(|p| (p.id, p.pid)).unwrap_or_default();
    let pidfd = rustix::process::Pid::from_raw(pid).and_then(|p| rustix::process::pidfd_open(p, rustix::process::PidfdFlags::empty()).ok());
    if let Ok(w) = s.try_clone() {
        std::thread::spawn(move || forward(set, w));
    }
    let Some(exited) = read(&mut r) else {
        if let Some(fd) = pidfd {
            let mut fds = [rustix::event::PollFd::new(&fd, rustix::event::PollFlags::IN)];
            while !matches!(rustix::event::poll(&mut fds, None), Ok(n) if n > 0) {}
        }
        eprintln!("acfs: lost the daemon while the process ran (acfsd restarted?); its exit status is unknown");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3600);
        while std::time::Instant::now() < deadline {
            let state = crate::call(socket, &Request::ProcShow { proc_id: id.clone() })
                .ok()
                .and_then(|r| r.result.get("state").and_then(|s| s.as_str()).map(str::to_owned));
            match state.as_deref() {
                Some("running" | "exiting") | None => std::thread::sleep(std::time::Duration::from_millis(200)),
                Some(st) => {
                    eprintln!("acfs: process {id} is {st}");
                    break;
                }
            }
        }
        std::process::exit(1);
    };
    if !exited.ok {
        error(exited);
    }
    let e: ProcExited = serde_json::from_value(exited.result).unwrap_or_else(|e| fail(format!("bad result: {e}")));
    for m in e.merge.results.iter().filter(|m| m.status == MergeStatus::Conflict) {
        let paths: Vec<String> =
            m.conflicts.iter().map(|c| format!("{} ({})", String::from_utf8_lossy(&c.path.to_bytes().unwrap_or_default()), c.reason)).collect();
        eprintln!("acfs: process {} left conflicts in {}: {}; settle them with `acfs proc merge {}`", e.id, m.ws, paths.join(", "), e.id);
    }
    if let Some(st) = &e.merge.resolver {
        eprintln!("acfs: resolver: {st}");
    }
    std::process::exit(match (e.code, e.signal) {
        (Some(c), _) => c,
        (None, Some(s)) => 128 + s,
        _ => 1,
    })
}
