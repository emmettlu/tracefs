mod run;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use acfs_core::path::{PathName, b64_decode, b64_encode};
use acfs_core::proto::{
    Changeset, Diff, ErrorBody, FsAt, FsEdit, FsList, FsOp, FsRead, FsStat, Log, ProcChangeset, ProcMerge, Remote, Request, Reset, ResolutionAction,
    ResolutionSpec, Response, Revert, SessionCreate, WsPrune,
};
use acfs_core::results::FsFile;
use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Parser)]
#[command(name = "acfs", version, about = "AgentComputerFS: versioned agent workspaces (client for acfs-daemon)")]
struct Cli {
    #[arg(long, env = "ACFS_SOCKET", default_value = "/run/acfs/acfs-daemon.sock", global = true)]
    socket: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    #[command(about = "Workspaces")]
    #[command(subcommand)]
    Ws(WsCmd),
    #[command(about = "Sessions")]
    #[command(subcommand)]
    Session(SessionCmd),
    #[command(
        about = "Run a command (an agent) on its own branch of every workspace; its work is merged into main when it is done",
        long_about = "Run a command as you, in its own mount namespace where each workspace is its own copy-on-write branch.\nIts work is merged into main when it calls `acfs proc merge` (a task is done) and when it exits; conflicts\ngo to the --resolver command. Exits with the command's status."
    )]
    Run {
        #[arg(help = "Workspaces to branch (repeatable; default: all of yours)")]
        #[arg(long)]
        ws: Vec<String>,
        #[arg(help = "Shell command that settles merge conflicts (run in a directory with TASK.md)")]
        #[arg(long)]
        resolver: Option<String>,
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
    #[command(about = "Processes started with `acfs run`")]
    #[command(subcommand)]
    Proc(ProcCmd),
    #[command(about = "Snapshot workspaces now (their main: the directory as it is)")]
    Changeset(Changeset),
    #[command(about = "Bring a branch back to a changeset or snapshot (recorded as a new changeset)")]
    Reset(Reset),
    #[command(about = "Undo a changeset, everything a session recorded, or everything after --from up to --to")]
    Revert {
        changeset: Option<String>,
        #[arg(long, conflicts_with = "changeset")]
        session: Option<String>,
        #[arg(long, requires = "to", conflicts_with_all = ["changeset", "session"])]
        from: Option<String>,
        #[arg(long, requires = "from")]
        to: Option<String>,
        #[arg(long)]
        resolutions: Option<PathBuf>,
    },
    #[command(about = "Changeset history of a process, session or workspace")]
    Log(Log),
    #[command(about = "Changes of a changeset/snapshot against its parent, or between two")]
    Diff(Diff),
    #[command(about = "Send the workspace's main history to a remote directory")]
    Push(Remote),
    #[command(about = "Receive a workspace's main history from a remote directory")]
    Pull(Remote),
    #[command(about = "Base layers")]
    #[command(subcommand)]
    Base(BaseCmd),
    #[command(about = "Files of a workspace, without mounting it")]
    #[command(subcommand)]
    Fs(FsCmd),
    #[command(about = "Check that the daemon answers")]
    Ping,
}

#[derive(Subcommand)]
enum WsCmd {
    #[command(about = "Take over a directory: mount the workspace over it and record every change made there")]
    Register {
        name: String,
        #[arg(long)]
        from: String,
    },
    List,
    #[command(about = "Stop tracking: write the head back to the directory and unmount")]
    Detach {
        ws: String,
    },
    #[command(about = "Thin out a workspace's old automatic snapshots now")]
    Prune(WsPrune),
}

#[derive(Subcommand)]
enum SessionCmd {
    Create(SessionCreate),
    List,
    #[command(about = "End a session; its workspaces keep their content")]
    Merge {
        session: String,
    },
    #[command(about = "End a session and bring its workspaces back to where it started")]
    Discard {
        session: String,
    },
}

#[derive(Subcommand)]
enum ProcCmd {
    Changeset(ProcChangeset),
    #[command(about = "The task is done: merge the process's branches into main and bring main's other changes in")]
    Merge {
        proc_id: String,
        #[arg(long)]
        resolutions: Option<PathBuf>,
        #[arg(long = "discard-ws")]
        discard_ws: Vec<String>,
        #[arg(help = "Report conflicts instead of running the resolver")]
        #[arg(long)]
        no_resolver: bool,
    },
    #[command(about = "Signal a running process's group")]
    Kill {
        proc_id: String,
        #[arg(long)]
        signal: Option<i32>,
    },
    Show {
        proc_id: String,
    },
    List,
}

#[derive(Subcommand)]
enum BaseCmd {
    #[command(about = "Promote main's head snapshot to a new base layer")]
    Promote { snap: String },
    #[command(about = "Squash the workspace's base layers and main's changes into one layer")]
    Flatten { ws: String },
}

#[derive(clap::Args)]
#[command(about = "How `fs` changes are recorded")]
struct EditArgs {
    #[arg(help = "Refuse if a touched path changed since this changeset or snapshot")]
    #[arg(long)]
    base: Option<String>,
    #[arg(long)]
    author: Option<String>,
    #[arg(short = 'm', long)]
    message: Option<String>,
    #[arg(long)]
    tool_call: Option<String>,
}

fn octal(s: &str) -> Result<u32, String> {
    u32::from_str_radix(s, 8).map_err(|e| e.to_string())
}

#[derive(Subcommand)]
enum FsCmd {
    #[command(about = "Print a file (by default from the workspace's directory as it is now)")]
    Cat {
        #[command(flatten)]
        at: FsAt,
        path: String,
    },
    #[command(about = "List a directory")]
    Ls {
        #[command(flatten)]
        at: FsAt,
        path: Option<String>,
    },
    Stat {
        #[command(flatten)]
        at: FsAt,
        path: String,
    },
    #[command(about = "Write a file from --file or standard input")]
    Write {
        ws: String,
        path: String,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long, value_parser = octal)]
        mode: Option<u32>,
        #[command(flatten)]
        edit: EditArgs,
    },
    #[command(about = "Remove entries (directories with their contents)")]
    Rm {
        ws: String,
        #[arg(required = true)]
        paths: Vec<String>,
        #[command(flatten)]
        edit: EditArgs,
    },
    Mkdir {
        ws: String,
        path: String,
        #[arg(long, value_parser = octal)]
        mode: Option<u32>,
        #[command(flatten)]
        edit: EditArgs,
    },
    #[command(about = "Rename an entry; the destination must not exist")]
    Mv {
        ws: String,
        from: String,
        to: String,
        #[command(flatten)]
        edit: EditArgs,
    },
    #[command(about = "Create a symlink at `path` pointing to `target`")]
    Ln {
        ws: String,
        target: String,
        path: String,
        #[command(flatten)]
        edit: EditArgs,
    },
    Chmod {
        ws: String,
        #[arg(value_parser = octal)]
        mode: u32,
        path: String,
        #[command(flatten)]
        edit: EditArgs,
    },
    #[command(about = "Apply a JSON list of ops (`fs.edit`) as one changeset; a write op may name a local `content_path` instead of base64 `content`")]
    Edit {
        ws: String,
        #[arg(long)]
        ops: PathBuf,
        #[command(flatten)]
        edit: EditArgs,
    },
}

fn fs_edit(ws: String, ops: Vec<FsOp>, e: EditArgs) -> Request {
    Request::FsEdit(FsEdit { ws, base: e.base, ops, author: e.author, message: e.message, tool_call: e.tool_call })
}

fn ops_file(p: &PathBuf) -> Result<Vec<FsOp>> {
    let mut items: Vec<Value> = serde_json::from_slice(&std::fs::read(p).with_context(|| format!("read {}", p.display()))?)?;
    for it in &mut items {
        if let Some(cp) = it.get("content_path").and_then(Value::as_str).map(PathBuf::from) {
            let data = std::fs::read(&cp).with_context(|| format!("read {}", cp.display()))?;
            it["content"] = json!(b64_encode(&data));
            it.as_object_mut().unwrap().remove("content_path");
        }
    }
    Ok(serde_json::from_value(Value::Array(items))?)
}

fn read_input(file: &Option<PathBuf>) -> Result<Vec<u8>> {
    Ok(match file {
        Some(f) => std::fs::read(f).with_context(|| format!("read {}", f.display()))?,
        None => {
            let mut v = Vec::new();
            std::io::Read::read_to_end(&mut std::io::stdin(), &mut v)?;
            v
        }
    })
}

fn fs_request(cmd: FsCmd) -> Result<Request> {
    let p = |s: String| PathName::Utf8(s);
    Ok(match cmd {
        FsCmd::Cat { at, path } => Request::FsRead(FsRead { at, path: p(path), offset: None, length: None }),
        FsCmd::Ls { at, path } => Request::FsList(FsList { at, path: path.map(p) }),
        FsCmd::Stat { at, path } => Request::FsStat(FsStat { at, path: p(path) }),
        FsCmd::Write { ws, path, file, mode, edit } => fs_edit(ws, vec![FsOp::Write { path: p(path), content: b64_encode(&read_input(&file)?), mode }], edit),
        FsCmd::Rm { ws, paths, edit } => fs_edit(ws, paths.into_iter().map(|x| FsOp::Delete { path: p(x) }).collect(), edit),
        FsCmd::Mkdir { ws, path, mode, edit } => fs_edit(ws, vec![FsOp::Mkdir { path: p(path), mode }], edit),
        FsCmd::Mv { ws, from, to, edit } => fs_edit(ws, vec![FsOp::Rename { from: p(from), to: p(to) }], edit),
        FsCmd::Ln { ws, target, path, edit } => fs_edit(ws, vec![FsOp::Symlink { path: p(path), target: p(target) }], edit),
        FsCmd::Chmod { ws, mode, path, edit } => fs_edit(ws, vec![FsOp::Chmod { path: p(path), mode }], edit),
        FsCmd::Edit { ws, ops, edit } => fs_edit(ws, ops_file(&ops)?, edit),
    })
}

#[derive(Deserialize)]
struct ResolutionFile {
    ws: String,
    path: PathName,
    action: ResolutionAction,
    #[serde(default)]
    content_path: Option<PathBuf>,
}

fn resolutions(p: &Option<PathBuf>) -> Result<Vec<ResolutionSpec>> {
    let Some(p) = p else { return Ok(Vec::new()) };
    let items: Vec<ResolutionFile> = serde_json::from_slice(&std::fs::read(p).with_context(|| format!("read {}", p.display()))?)?;
    items
        .into_iter()
        .map(|r| {
            let content = match &r.content_path {
                Some(cp) => Some(b64_encode(&std::fs::read(cp).with_context(|| format!("read {}", cp.display()))?)),
                None => None,
            };
            if r.action == ResolutionAction::Content && content.is_none() {
                bail!("resolution for {:?} needs content_path", r.path);
            }
            Ok(ResolutionSpec { ws: r.ws, path: r.path, action: r.action, content })
        })
        .collect()
}

fn canonical(p: &str) -> Result<String> {
    let c = std::fs::canonicalize(p).with_context(|| p.to_string())?;
    c.into_os_string().into_string().map_err(|c| anyhow!("{} is not UTF-8", c.to_string_lossy()))
}

fn call(socket: &Path, req: &Request) -> Result<Response> {
    let mut s = UnixStream::connect(socket).with_context(|| format!("connect {}", socket.display()))?;
    let mut line = serde_json::to_vec(req)?;
    line.push(b'\n');
    s.write_all(&line)?;
    let mut buf = String::new();
    BufReader::new(&s).read_line(&mut buf)?;
    serde_json::from_str(&buf).map_err(|e| anyhow!("bad response: {e}"))
}

fn request(cmd: Cmd) -> Result<Request> {
    Ok(match cmd {
        Cmd::Ping => Request::Ping,
        Cmd::Ws(WsCmd::Register { name, from }) => Request::WsRegister { name, from: canonical(&from)? },
        Cmd::Ws(WsCmd::List) => Request::WsList,
        Cmd::Ws(WsCmd::Detach { ws }) => Request::WsDetach { ws },
        Cmd::Ws(WsCmd::Prune(a)) => Request::WsPrune(a),
        Cmd::Fs(c) => fs_request(c)?,
        Cmd::Changeset(a) => Request::Changeset(a),
        Cmd::Reset(a) => Request::Reset(a),
        Cmd::Revert { changeset, session, from, to, resolutions: r } => Request::Revert(Revert { changeset, session, from, to, resolutions: resolutions(&r)? }),
        Cmd::Session(SessionCmd::Create(a)) => Request::SessionCreate(a),
        Cmd::Session(SessionCmd::List) => Request::SessionList,
        Cmd::Session(SessionCmd::Merge { session }) => Request::SessionMerge { session },
        Cmd::Session(SessionCmd::Discard { session }) => Request::SessionDiscard { session },
        Cmd::Proc(ProcCmd::Changeset(a)) => Request::ProcChangeset(a),
        Cmd::Proc(ProcCmd::Merge { proc_id, resolutions: r, discard_ws, no_resolver }) => {
            Request::ProcMerge(ProcMerge { proc_id, resolutions: resolutions(&r)?, discard_ws, no_resolver })
        }
        Cmd::Proc(ProcCmd::Kill { proc_id, signal }) => Request::ProcKill { proc_id, signal },
        Cmd::Proc(ProcCmd::Show { proc_id }) => Request::ProcShow { proc_id },
        Cmd::Proc(ProcCmd::List) => Request::ProcList,
        Cmd::Run { .. } => unreachable!(),
        Cmd::Log(a) => Request::Log(a),
        Cmd::Diff(a) => Request::Diff(a),
        Cmd::Push(a) => Request::Push(a),
        Cmd::Pull(a) => Request::Pull(a),
        Cmd::Base(BaseCmd::Promote { snap }) => Request::BasePromote { snap },
        Cmd::Base(BaseCmd::Flatten { ws }) => Request::BaseFlatten { ws },
    })
}

fn main() {
    let cli = Cli::parse();
    let cmd = match cli.cmd {
        Cmd::Run { ws, resolver, cmd } => run::main(&cli.socket, ws, resolver, cmd),
        c => c,
    };
    let cat = matches!(&cmd, Cmd::Fs(FsCmd::Cat { .. }));
    let res = request(cmd).and_then(|req| call(&cli.socket, &req));
    let r = match res {
        Ok(r) => r,
        Err(e) => {
            eprintln!("acfs: {e:#}");
            std::process::exit(1);
        }
    };
    if !r.ok {
        let e = r.error.unwrap_or(ErrorBody { code: "internal".into(), message: "unknown error".into() });
        eprintln!("{}", json!({ "error": e }));
        std::process::exit(1);
    }
    if cat {
        let f: FsFile = serde_json::from_value(r.result).unwrap_or_else(|e| {
            eprintln!("acfs: bad read result: {e}");
            std::process::exit(1);
        });
        let data = match (f.content, f.target) {
            (Some(c), _) => b64_decode(&c).unwrap_or_default(),
            (None, Some(t)) => t.to_bytes().unwrap_or_default(),
            _ => Vec::new(),
        };
        let _ = std::io::stdout().write_all(&data);
        return;
    }
    println!("{}", serde_json::to_string_pretty(&r.result).unwrap_or_default());
    if r.result.get("conflict").and_then(Value::as_bool) == Some(true) {
        std::process::exit(2);
    }
}
