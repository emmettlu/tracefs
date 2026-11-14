use std::collections::{HashMap, HashSet};
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::Connection;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::process::{Pid, PidfdFlags, pidfd_open};

use crate::daemon::rows;

pub struct Caller<'a> {
    pub uid: u32,
    pub gid: u32,
    pub pid: i32,
    pub sock: BorrowedFd<'a>,
}

impl Caller<'_> {
    pub fn pidfd(&self) -> io::Result<OwnedFd> {
        let mut fd: libc::c_int = -1;
        let mut len = size_of::<libc::c_int>() as libc::socklen_t;
        let r = unsafe { libc::getsockopt(self.sock.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERPIDFD, (&raw mut fd).cast(), &mut len) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

pub fn start_time(pid: i32) -> Option<u64> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    s.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
}

pub fn pidfd_of(pid: i32, start: u64) -> Option<OwnedFd> {
    let fd = pidfd_open(Pid::from_raw(pid)?, PidfdFlags::empty()).ok()?;
    (start_time(pid) == Some(start)).then_some(fd)
}

pub fn wait_exit(pidfd: &OwnedFd) {
    let mut fds = [PollFd::new(pidfd, PollFlags::IN)];
    while !matches!(poll(&mut fds, None), Ok(n) if n > 0) {}
}

pub fn exited(pidfd: &OwnedFd) -> bool {
    let mut fds = [PollFd::new(pidfd, PollFlags::IN)];
    poll(&mut fds, Some(&Timespec { tv_sec: 0, tv_nsec: 0 })).is_ok_and(|n| n > 0)
}

#[derive(Default)]
pub struct Callers(Mutex<HashMap<String, Option<OwnedFd>>>);

impl Callers {
    pub fn bind(&self, session: &str, pidfd: OwnedFd) {
        self.0.lock().unwrap().insert(session.to_string(), Some(pidfd));
    }

    pub fn forget(&self, session: &str) {
        self.0.lock().unwrap().remove(session);
    }

    pub fn gone(&self) -> HashSet<String> {
        let mut m = self.0.lock().unwrap();
        for fd in m.values_mut() {
            if fd.as_ref().is_some_and(exited) {
                *fd = None;
            }
        }
        m.iter().filter(|(_, fd)| fd.is_none()).map(|(s, _)| s.clone()).collect()
    }

    pub fn load(&self, c: &Connection) -> Result<()> {
        let sql = "SELECT id, caller_pid, caller_start FROM sessions WHERE state = 'active' AND caller_pid IS NOT NULL";
        let bound: Vec<(String, i32, i64)> = rows(c, sql, [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let mut m = self.0.lock().unwrap();
        for (session, pid, start) in bound {
            m.insert(session, pidfd_of(pid, start as u64));
        }
        Ok(())
    }
}
