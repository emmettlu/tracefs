use std::io::{BufRead, BufReader, IoSliceMut, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::ExitStatusExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags};
use serde_json::Value;
use tracefs_core::proto::{ErrorBody, PROTOCOL, ProcRun, Request, Response};
use tracefs_core::results::{Pong, ProcExited};

use crate::callers::Caller;
use crate::daemon::{ApiError, Daemon, invalid};
use crate::ops;

const MAX_REQUEST: u64 = 256 << 20;

pub fn dispatch(d: &Daemon, caller: &Caller, req: Request) -> Result<Value> {
    use ops::*;
    let uid = caller.uid;
    fn val<T: serde::Serialize>(r: Result<T>) -> Result<Value> {
        Ok(serde_json::to_value(r?)?)
    }
    match req {
        Request::Ping => val(Ok(Pong { version: env!("CARGO_PKG_VERSION").into(), protocol: PROTOCOL })),
        Request::WsRegister { name, from } => val(ws::register(d, uid, &name, &from)),
        Request::WsList => val(ws::list(d, uid)),
        Request::WsDetach { ws } => val(live::detach(d, uid, &ws)),
        Request::WsPrune(a) => val(retention::prune(d, uid, &a.ws, a.dry_run)),
        Request::Changeset(a) => val(live::changeset(d, uid, a)),
        Request::Reset(a) => val(live::reset(d, uid, a)),
        Request::Revert(a) => val(live::revert(d, uid, a)),
        Request::BasePromote { snap } => val(ws::promote(d, uid, &snap)),
        Request::BaseFlatten { ws } => val(ws::flatten(d, uid, &ws)),
        Request::SessionCreate(a) => val(session::create(d, caller, a)),
        Request::SessionList => val(session::list(d, uid)),
        Request::SessionMerge { session } => val(session::end(d, uid, &session, false)),
        Request::SessionDiscard { session } => val(session::end(d, uid, &session, true)),
        Request::ProcRun(_) => Err(invalid("proc.run passes its standard streams: use the tracefs CLI (`tracefs run`)")),
        Request::ProcChangeset(a) => val(procs::changeset(d, uid, a)),
        Request::ProcMerge(a) => val(procs::merge(d, uid, a)),
        Request::ProcKill { proc_id, signal } => val(procs::kill(d, uid, &proc_id, signal)),
        Request::ProcShow { proc_id } => val(procs::show(d, uid, &proc_id)),
        Request::ProcList => val(procs::list(d, uid)),
        Request::Log(a) => val(history::log(d, uid, a)),
        Request::Diff(a) => val(history::diff(d, uid, a)),
        Request::Push(a) => val(remote::push(d, uid, a)),
        Request::Pull(a) => val(remote::pull(d, uid, a)),
        Request::FsRead(a) => val(fs::read(d, uid, a)),
        Request::FsList(a) => val(fs::list(d, uid, a)),
        Request::FsStat(a) => val(fs::stat(d, uid, a)),
        Request::FsEdit(a) => val(fs::edit(d, uid, a)),
    }
}

fn err_response(e: &anyhow::Error) -> Response {
    let code = e.downcast_ref::<ApiError>().map(|a| a.code).unwrap_or("internal");
    Response { ok: false, result: Value::Null, error: Some(ErrorBody { code: code.into(), message: format!("{e:#}") }) }
}

fn respond(line: &str, d: &Daemon, caller: &Caller) -> Response {
    let req: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => return err_response(&invalid(format!("bad request: {e}"))),
    };
    let op = op_of(line);
    let t0 = std::time::Instant::now();
    let r = dispatch(d, caller, req);
    let ms = t0.elapsed().as_millis();
    match r {
        Ok(result) => {
            eprintln!("tracefs-daemon: {op} ok ({ms} ms)");
            Response { ok: true, result, error: None }
        }
        Err(e) => {
            eprintln!("tracefs-daemon: {op} failed ({ms} ms): {e:#}");
            err_response(&e)
        }
    }
}

fn op_of(line: &str) -> String {
    #[derive(serde::Deserialize)]
    struct Head {
        op: String,
    }
    serde_json::from_str::<Head>(line).map(|h| h.op).unwrap_or_default()
}

fn read_request(s: &UnixStream) -> Result<(String, Vec<OwnedFd>)> {
    let mut data = Vec::new();
    let mut fds = Vec::new();
    let mut buf = vec![0u8; 64 << 10];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(3))];
    while !data.contains(&b'\n') {
        let mut control = RecvAncillaryBuffer::new(&mut space);
        let r = rustix::net::recvmsg(s, &mut [IoSliceMut::new(&mut buf)], &mut control, RecvFlags::CMSG_CLOEXEC)?;
        for m in control.drain() {
            if let RecvAncillaryMessage::ScmRights(got) = m {
                fds.extend(got);
            }
        }
        if r.bytes == 0 {
            break;
        }
        data.extend_from_slice(&buf[..r.bytes]);
        if data.len() as u64 > MAX_REQUEST {
            return Err(anyhow!("request too large"));
        }
    }
    let end = data.iter().position(|&b| b == b'\n').unwrap_or(data.len());
    data.truncate(end);
    Ok((String::from_utf8(data)?, fds))
}

fn send(s: &UnixStream, resp: &Response) {
    let mut out = serde_json::to_vec(resp).unwrap_or_default();
    out.push(b'\n');
    let _ = (&*s).write_all(&out);
}

fn ok<T: serde::Serialize>(v: &T) -> Response {
    Response { ok: true, result: serde_json::to_value(v).unwrap_or_default(), error: None }
}

fn run_conn(d: &Daemon, caller: &Caller, line: &str, fds: Vec<OwnedFd>, s: &UnixStream) {
    let a: ProcRun = match serde_json::from_str::<Request>(line) {
        Ok(Request::ProcRun(a)) => a,
        _ => return send(s, &err_response(&invalid("bad proc.run request"))),
    };
    let (started, mut child) = match crate::ops::procs::start(d, caller, &a, fds) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("tracefs-daemon: proc.run failed: {e:#}");
            return send(s, &err_response(&e));
        }
    };
    eprintln!("tracefs-daemon: proc.run {} started (pid {})", started.id, started.pid);
    let (id, pid) = (started.id.clone(), started.pid);
    send(s, &ok(&started));
    let done = Arc::new(AtomicBool::new(false));
    if let Ok(r) = s.try_clone() {
        let done = done.clone();
        let stop = Duration::from_millis(d.cfg.stop_timeout_ms);
        let scope = crate::ops::procs::cgroup_kill_path(d, &id);
        std::thread::spawn(move || {
            for line in BufReader::new(r).lines() {
                let Ok(line) = line else { break };
                let sig = serde_json::from_str::<Value>(&line).ok().and_then(|v| v["signal"].as_i64());
                if let Some(sig) = sig
                    && !done.load(Ordering::SeqCst)
                {
                    crate::ops::procs::signal_group(pid, sig as i32);
                }
            }
            if done.load(Ordering::SeqCst) {
                return;
            }
            crate::ops::procs::signal_group(pid, 15);
            std::thread::sleep(stop);
            if !done.load(Ordering::SeqCst) {
                let _ = std::fs::write(&scope, "1");
            }
        });
    }
    let status = child.wait();
    done.store(true, Ordering::SeqCst);
    let (code, signal) = match &status {
        Ok(st) => (st.code(), st.signal()),
        Err(_) => (None, None),
    };
    let resp = match crate::ops::procs::exited(d, &id) {
        Ok(merge) => {
            eprintln!("tracefs-daemon: process {id} exited ({code:?}/{signal:?}), {}", merge.state);
            ok(&ProcExited { id, code, signal, merge })
        }
        Err(e) => {
            eprintln!("tracefs-daemon: settle process {id}: {e:#}");
            err_response(&e)
        }
    };
    send(s, &resp);
}

fn handle(d: &Daemon, s: UnixStream) {
    let caller = match rustix::net::sockopt::socket_peercred(&s) {
        Ok(c) => Caller { uid: c.uid.as_raw(), gid: c.gid.as_raw(), pid: c.pid.as_raw_nonzero().get(), sock: s.as_fd() },
        Err(e) => return eprintln!("tracefs-daemon: peer credentials: {e}"),
    };
    let (line, fds) = match read_request(&s) {
        Ok(x) => x,
        Err(e) => return send(&s, &err_response(&invalid(e.to_string()))),
    };
    if op_of(&line) == "proc.run" {
        return run_conn(d, &caller, &line, fds, &s);
    }
    drop(fds);
    send(&s, &respond(&line, d, &caller));
}

fn notify_ready() {
    if let Some(p) = std::env::var_os("NOTIFY_SOCKET")
        && let Ok(s) = std::os::unix::net::UnixDatagram::unbound()
    {
        let _ = s.send_to(b"READY=1", p);
    }
}

pub fn serve(d: Arc<Daemon>) -> Result<()> {
    let sock = &d.cfg.socket.clone().unwrap_or_else(|| d.paths.run.join("tracefs-daemon.sock"));
    let _ = std::fs::remove_file(sock);
    let l = UnixListener::bind(sock).with_context(|| format!("bind {}", sock.display()))?;
    std::fs::set_permissions(sock, std::fs::Permissions::from_mode(d.cfg.socket_mode & 0o777))?;
    notify_ready();
    eprintln!("tracefs-daemon: listening on {}", sock.display());
    for s in l.incoming() {
        match s {
            Ok(s) => {
                let d = d.clone();
                std::thread::spawn(move || handle(&d, s));
            }
            Err(e) => eprintln!("tracefs-daemon: accept: {e}"),
        }
    }
    Ok(())
}
