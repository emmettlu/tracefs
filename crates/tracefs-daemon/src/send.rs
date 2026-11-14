use std::collections::{BTreeSet, HashMap};
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use tracefs_core::path::{self, RelPath};

const C_SUBVOL: u16 = 1;
const C_SNAPSHOT: u16 = 2;
const C_CREATE: [u16; 7] = [3, 4, 5, 6, 7, 8, 10];
const C_RENAME: u16 = 9;
const C_END: u16 = 21;
const A_PATH: u16 = 15;
const A_PATH_TO: u16 = 16;
const A_PATH_LINK: u16 = 17;
const A_DATA: u16 = 19;

pub fn touched_upper(a: &Path, b: &Path) -> Result<BTreeSet<RelPath>> {
    let mut child = Command::new("btrfs")
        .args(["send", "-q", "--no-data", "-p"])
        .arg(a)
        .arg(b)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("run btrfs send")?;
    let parsed = parse(BufReader::with_capacity(1 << 20, child.stdout.take().unwrap()));
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err)?;
    let status = child.wait()?;
    let ops = parsed.context("parse btrfs send stream")?;
    if !status.success() {
        bail!("btrfs send failed ({status}): {}", err.trim());
    }
    Ok(candidates(ops).into_iter().filter_map(|p| if p == b"upper" { Some(Vec::new()) } else { p.strip_prefix(b"upper/").map(<[u8]>::to_vec) }).collect())
}

#[derive(Debug, Default, PartialEq)]
struct Op {
    path: Option<RelPath>,
    to: Option<RelPath>,
    link: Option<RelPath>,
    rename: bool,
    create: bool,
}

fn read_exact(r: &mut impl Read, n: usize) -> Result<Option<Vec<u8>>> {
    let mut buf = vec![0u8; n];
    let mut got = 0;
    while got < n {
        match r.read(&mut buf[got..])? {
            0 if got == 0 => return Ok(None),
            0 => bail!("truncated send stream"),
            k => got += k,
        }
    }
    Ok(Some(buf))
}

fn parse(mut r: impl Read) -> Result<Vec<Op>> {
    let head = read_exact(&mut r, 17)?.context("empty send stream")?;
    if &head[..13] != b"btrfs-stream\0" {
        bail!("not a btrfs send stream");
    }
    let version = u32::from_le_bytes(head[13..17].try_into().unwrap());
    let mut ops = Vec::new();
    loop {
        let Some(h) = read_exact(&mut r, 10)? else { break };
        let len = u32::from_le_bytes(h[0..4].try_into().unwrap()) as usize;
        let cmd = u16::from_le_bytes(h[4..6].try_into().unwrap());
        let body = read_exact(&mut r, len)?.context("truncated send command")?;
        if cmd == C_END {
            break;
        }
        if cmd == C_SUBVOL || cmd == C_SNAPSHOT {
            continue;
        }
        let mut op = Op { rename: cmd == C_RENAME, create: C_CREATE.contains(&cmd), ..Op::default() };
        let mut at = 0;
        while at + 4 <= body.len() {
            let ty = u16::from_le_bytes(body[at..at + 2].try_into().unwrap());
            if version >= 2 && ty == A_DATA {
                break;
            }
            let n = u16::from_le_bytes(body[at + 2..at + 4].try_into().unwrap()) as usize;
            let v = body.get(at + 4..at + 4 + n).context("truncated send attribute")?.to_vec();
            at += 4 + n;
            match ty {
                A_PATH => op.path = Some(v),
                A_PATH_TO => op.to = Some(v),
                A_PATH_LINK => op.link = Some(v),
                _ => {}
            }
        }
        if op.path.is_some() || op.to.is_some() || op.link.is_some() {
            ops.push(op);
        }
    }
    std::io::copy(&mut r, &mut std::io::sink())?;
    Ok(ops)
}

struct Replay {
    nodes: Vec<(Option<usize>, RelPath, Option<RelPath>)>,
    children: HashMap<(usize, RelPath), usize>,
    touched: Vec<usize>,
}

impl Replay {
    fn new() -> Replay {
        Replay { nodes: vec![(None, Vec::new(), Some(Vec::new()))], children: HashMap::new(), touched: Vec::new() }
    }

    fn lookup(&mut self, p: &[u8], create: bool) -> usize {
        let names: Vec<&[u8]> = p.split(|&c| c == b'/').filter(|n| !n.is_empty()).collect();
        let mut cur = 0;
        for (k, name) in names.iter().enumerate() {
            let key = (cur, name.to_vec());
            cur = match self.children.get(&key) {
                Some(&n) => n,
                None => {
                    let in_a = if create && k + 1 == names.len() { None } else { self.nodes[cur].2.as_ref().map(|a| path::join(a, name)) };
                    self.nodes.push((Some(cur), name.to_vec(), in_a));
                    self.children.insert(key, self.nodes.len() - 1);
                    self.nodes.len() - 1
                }
            };
        }
        cur
    }

    fn touch(&mut self, p: &[u8], create: bool) -> usize {
        let n = self.lookup(p, create);
        self.touched.push(n);
        n
    }

    fn detach(&mut self, n: usize) {
        if let Some(parent) = self.nodes[n].0.take() {
            self.children.remove(&(parent, self.nodes[n].1.clone()));
        }
    }

    fn apply(&mut self, op: &Op) {
        match (&op.path, &op.to) {
            (Some(from), Some(to)) if op.rename => {
                let n = self.touch(from, false);
                let old = self.touch(to, false);
                if old != n {
                    self.detach(old);
                }
                self.detach(n);
                let parent = self.lookup(path::parent(to), false);
                let name = path::file_name(to).to_vec();
                self.nodes[n].0 = Some(parent);
                self.nodes[n].1 = name.clone();
                self.children.insert((parent, name), n);
            }
            _ => {
                if let Some(p) = &op.path {
                    self.touch(p, op.create);
                }
                if let Some(p) = &op.link {
                    self.touch(p, false);
                }
            }
        }
    }

    fn in_b(&self, mut n: usize) -> Option<RelPath> {
        let mut names = Vec::new();
        while n != 0 {
            let (parent, name, _) = &self.nodes[n];
            names.push(name.as_slice());
            n = (*parent)?;
        }
        names.reverse();
        Some(names.join(&b'/'))
    }
}

fn candidates(ops: Vec<Op>) -> BTreeSet<RelPath> {
    let mut r = Replay::new();
    for op in &ops {
        r.apply(op);
    }
    let mut out = BTreeSet::new();
    for &n in &r.touched {
        out.extend(r.nodes[n].2.clone());
        out.extend(r.in_b(n));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(c: u16, attrs: &[(u16, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (t, v) in attrs {
            body.extend_from_slice(&t.to_le_bytes());
            body.extend_from_slice(&(v.len() as u16).to_le_bytes());
            body.extend_from_slice(v);
        }
        let mut out = (body.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(&c.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend(body);
        out
    }

    fn strings(s: &BTreeSet<RelPath>) -> Vec<String> {
        s.iter().map(|p| String::from_utf8_lossy(p).into_owned()).collect()
    }

    #[test]
    fn parse_and_map() {
        let mut s = b"btrfs-stream\0".to_vec();
        s.extend_from_slice(&1u32.to_le_bytes());
        s.extend(cmd(2, &[(15, b"s2"), (1, &[0; 16])]));
        s.extend(cmd(9, &[(15, b"upper/d"), (16, b"upper/e")]));
        s.extend(cmd(18, &[(15, b"upper/e/a"), (5, &[0; 8])]));
        s.extend(cmd(11, &[(15, b"upper/x")]));
        s.extend(cmd(4, &[(15, b"o263-10-0"), (3, &[0; 8])]));
        s.extend(cmd(9, &[(15, b"o263-10-0"), (16, b"upper/n")]));
        s.extend(cmd(3, &[(15, b"o264-10-0")]));
        s.extend(cmd(9, &[(15, b"o264-10-0"), (16, b"upper/n/z")]));
        s.extend(cmd(20, &[(15, b"upper")]));
        s.extend(cmd(21, &[]));
        let ops = parse(&s[..]).unwrap();
        assert_eq!(ops.len(), 8, "the snapshot command names no path");
        let c = candidates(ops);
        assert_eq!(strings(&c), vec!["upper", "upper/d", "upper/d/a", "upper/e", "upper/e/a", "upper/n", "upper/n/z", "upper/x"]);
    }

    #[test]
    fn rename_chains() {
        let ops = vec![
            Op { path: Some(b"a".to_vec()), to: Some(b"b".to_vec()), rename: true, ..Op::default() },
            Op { path: Some(b"b/f".to_vec()), ..Op::default() },
            Op { path: Some(b"b".to_vec()), to: Some(b"c".to_vec()), rename: true, ..Op::default() },
        ];
        assert_eq!(strings(&candidates(ops)), vec!["a", "a/f", "b", "c", "c/f"]);
    }

    #[test]
    fn replaced_then_moved_on() {
        let ops = vec![
            Op { path: Some(b"o1-1-0".to_vec()), create: true, ..Op::default() },
            Op { path: Some(b"o1-1-0".to_vec()), to: Some(b"x".to_vec()), rename: true, ..Op::default() },
            Op { path: Some(b"x".to_vec()), to: Some(b"y".to_vec()), rename: true, ..Op::default() },
        ];
        assert_eq!(strings(&candidates(ops)), vec!["x", "y"]);
    }

    #[test]
    fn many_copy_ups() {
        let mut ops = Vec::new();
        for i in 0..100_000 {
            let o = format!("o{i}-1-0").into_bytes();
            ops.push(Op { path: Some(o.clone()), create: true, ..Op::default() });
            ops.push(Op { path: Some(o), to: Some(format!("upper/d{}/f{i}", i % 100).into_bytes()), rename: true, ..Op::default() });
        }
        let t = std::time::Instant::now();
        assert_eq!(candidates(ops).len(), 100_000);
        assert!(t.elapsed().as_secs() < 5);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(&b"not a stream at all"[..]).is_err());
        let mut s = b"btrfs-stream\0".to_vec();
        s.extend_from_slice(&1u32.to_le_bytes());
        s.extend_from_slice(&[50, 0, 0, 0, 3, 0, 0, 0, 0, 0, 1]);
        assert!(parse(&s[..]).is_err());
    }
}
