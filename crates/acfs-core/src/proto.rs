use clap::Args;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::path::PathName;

pub const PROTOCOL: u32 = 2;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", content = "args")]
#[schemars(description = "`{\"op\": \"<name>\", \"args\": {...}}`.")]
pub enum Request {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "ws.register")]
    #[schemars(description = "Take over a directory: acfs mounts the workspace's main over it and records every change made there.")]
    WsRegister { name: String, from: String },
    #[serde(rename = "ws.list")]
    WsList,
    #[schemars(description = "Stop tracking: write the head back to the host directory and unmount.")]
    #[serde(rename = "ws.detach")]
    WsDetach { ws: String },
    #[schemars(description = "Thin out a workspace's automatic snapshots now (also done periodically).")]
    #[serde(rename = "ws.prune")]
    WsPrune(WsPrune),
    #[serde(rename = "changeset")]
    Changeset(Changeset),
    #[serde(rename = "reset")]
    Reset(Reset),
    #[serde(rename = "revert")]
    Revert(Revert),
    #[serde(rename = "base.promote")]
    BasePromote { snap: String },
    #[serde(rename = "base.flatten")]
    BaseFlatten { ws: String },
    #[serde(rename = "session.create")]
    SessionCreate(SessionCreate),
    #[serde(rename = "session.list")]
    SessionList,
    #[schemars(description = "End a session: its workspaces keep their content, the label stops.")]
    #[serde(rename = "session.merge")]
    SessionMerge { session: String },
    #[schemars(description = "End a session and bring its workspaces back to where it started.")]
    #[serde(rename = "session.discard")]
    SessionDiscard { session: String },
    #[schemars(
        description = "Run a command as the caller in its own mount namespace, every workspace replaced by\nthe process's own branch. CLI only: the request carries the caller's stdin, stdout and\nstderr (SCM_RIGHTS); a second response line (`ProcExited`) follows when it exits."
    )]
    #[serde(rename = "proc.run")]
    ProcRun(ProcRun),
    #[serde(rename = "proc.changeset")]
    ProcChangeset(ProcChangeset),
    #[schemars(
        description = "The process's task is done: snapshot its branches, merge them into main (the resolver\nhandles conflicts) and, while it runs, bring main's other changes into its branches."
    )]
    #[serde(rename = "proc.merge")]
    ProcMerge(ProcMerge),
    #[schemars(description = "Signal a running process's group (default SIGTERM).")]
    #[serde(rename = "proc.kill")]
    ProcKill {
        proc_id: String,
        #[serde(default)]
        signal: Option<i32>,
    },
    #[serde(rename = "proc.show")]
    ProcShow { proc_id: String },
    #[serde(rename = "proc.list")]
    ProcList,
    #[serde(rename = "log")]
    Log(Log),
    #[serde(rename = "diff")]
    Diff(Diff),
    #[serde(rename = "push")]
    Push(Remote),
    #[serde(rename = "pull")]
    Pull(Remote),
    #[schemars(description = "Read a file (or a symlink's target) of a workspace without mounting it.")]
    #[serde(rename = "fs.read")]
    FsRead(FsRead),
    #[schemars(description = "List a directory of a workspace.")]
    #[serde(rename = "fs.list")]
    FsList(FsList),
    #[serde(rename = "fs.stat")]
    FsStat(FsStat),
    #[schemars(description = "Change files of a workspace's main and record the change as one changeset.")]
    #[serde(rename = "fs.edit")]
    FsEdit(FsEdit),
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ErrorBody {
    #[schemars(description = "invalid, not_found, state (not allowed in the current state), exists, exhausted or internal.")]
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub result: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ResolutionAction {
    #[schemars(description = "Keep the target's version.")]
    Ours,
    #[schemars(description = "Take the source's version.")]
    Theirs,
    #[schemars(description = "Write the supplied content.")]
    Content,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Conflict resolution for one path.")]
pub struct ResolutionSpec {
    pub ws: String,
    pub path: PathName,
    pub action: ResolutionAction,
    #[schemars(description = "Base64 file content for action \"content\" (the CLI reads `content_path`).")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Args)]
pub struct SessionCreate {
    #[serde(default)]
    #[arg(long)]
    pub name: Option<String>,
    #[schemars(description = "Workspaces whose changes this session labels (repeatable).")]
    #[arg(help = "Workspaces whose changes this session labels (repeatable)")]
    #[serde(default)]
    #[arg(long)]
    pub ws: Vec<String>,
    #[schemars(description = "End (merge) the session after this many seconds without changes.")]
    #[arg(help = "End (merge) the session after this many seconds without changes")]
    #[serde(default)]
    #[arg(long)]
    pub idle_timeout: Option<u64>,
    #[schemars(
        description = "End (merge) the session once the calling process (the one that connected) exits,\nas if its idle timeout had passed. Needs Linux 6.5 (SO_PEERPIDFD). SDK only: the\nCLI process exits right away."
    )]
    #[arg(
        help = "End (merge) the session once the calling process (the one that connected) exits, as if its idle timeout had passed. Needs Linux 6.5 (SO_PEERPIDFD). SDK only: the CLI process exits right away"
    )]
    #[serde(default)]
    #[arg(skip)]
    pub end_on_exit: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Args)]
#[schemars(description = "Snapshot workspaces now (their main: the directory as it is).")]
#[command(about = "Snapshot workspaces now (their main: the directory as it is)")]
pub struct Changeset {
    #[schemars(description = "Workspaces; default: those of `--session`.")]
    #[arg(help = "Workspaces; default: those of `--session`")]
    #[serde(default)]
    pub ws: Vec<String>,
    #[serde(default)]
    #[arg(long)]
    pub session: Option<String>,
    #[serde(default)]
    #[arg(long)]
    pub author: Option<String>,
    #[serde(default)]
    #[arg(short = 'm', long)]
    pub message: Option<String>,
    #[serde(default)]
    #[arg(long)]
    pub tool_call: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Args)]
#[schemars(
    description = "Bring a branch back to a changeset or snapshot (recorded as a new changeset): a running\nprocess's branch for its own snapshots, else the workspace's main."
)]
#[command(about = "Bring a branch back to a changeset or snapshot (recorded as a new changeset)")]
pub struct Reset {
    #[schemars(description = "Changeset or snapshot id.")]
    #[arg(help = "Changeset or snapshot id")]
    pub target: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Undo one changeset, everything a session recorded, or everything between two states\n(`from`, `to`), on top of the current head.")]
pub struct Revert {
    #[serde(default)]
    pub changeset: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    #[schemars(description = "Changeset or snapshot the range starts at (not undone); needs `to`.")]
    #[serde(default)]
    pub from: Option<String>,
    #[schemars(description = "Changeset or snapshot the range ends at (undone), of the same workspaces as `from`.")]
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub resolutions: Vec<ResolutionSpec>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProcRun {
    #[schemars(description = "Workspaces the process gets a branch of; default: every attached workspace of the caller.")]
    #[serde(default)]
    pub ws: Vec<String>,
    pub cmd: Vec<String>,
    #[schemars(description = "The whole environment, `KEY=value`.")]
    #[serde(default)]
    pub env: Vec<String>,
    #[schemars(description = "Working directory (inside the process's view).")]
    pub cwd: String,
    #[schemars(description = "Command run as the caller, in a directory with `TASK.md`, when merging conflicts.")]
    #[serde(default)]
    pub resolver: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Args)]
pub struct ProcChangeset {
    pub proc_id: String,
    #[arg(long)]
    pub author: String,
    #[arg(short = 'm', long)]
    pub message: String,
    #[serde(default)]
    #[arg(long)]
    pub tool_call: Option<String>,
    #[schemars(description = "Record nothing when every branch still equals this changeset's snapshot of its workspace (default: the branch's head).")]
    #[serde(default)]
    #[arg(long)]
    pub since: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProcMerge {
    pub proc_id: String,
    #[serde(default)]
    pub resolutions: Vec<ResolutionSpec>,
    #[schemars(description = "Workspaces whose unmerged changes are dropped (the branch is reset to main).")]
    #[serde(default)]
    pub discard_ws: Vec<String>,
    #[schemars(description = "Report conflicts instead of running the resolver.")]
    #[serde(default)]
    pub no_resolver: bool,
}

#[derive(Debug, Serialize, Deserialize, Default, JsonSchema, Args)]
pub struct Log {
    #[serde(default)]
    #[arg(long = "proc")]
    pub proc_id: Option<String>,
    #[serde(default)]
    #[arg(long)]
    pub session: Option<String>,
    #[serde(default)]
    #[arg(long)]
    pub ws: Option<String>,
    #[serde(default)]
    #[arg(short)]
    pub n: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Args)]
pub struct Diff {
    pub a: String,
    #[serde(default)]
    pub b: Option<String>,
    #[serde(default)]
    #[arg(long)]
    pub patch: bool,
    #[schemars(description = "Compare complete views instead of the upper-layer shortcut (slow; for verification).")]
    #[arg(help = "Compare complete views instead of the upper-layer shortcut (slow; for verification)")]
    #[serde(default)]
    #[arg(long)]
    pub full: bool,
    #[schemars(description = "Also list paths that `.acfsignore` leaves out.")]
    #[arg(help = "Also list paths that `.acfsignore` leaves out")]
    #[serde(default)]
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Args)]
pub struct Remote {
    pub ws: String,
    #[serde(default)]
    #[arg(long)]
    pub remote: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Args)]
pub struct WsPrune {
    pub ws: String,
    #[schemars(description = "Only report what would be dropped.")]
    #[arg(help = "Only report what would be dropped")]
    #[serde(default)]
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Args)]
#[schemars(description = "Which state of a workspace to read: by default main's (its directory as it is now),\nelse a changeset or snapshot.")]
#[command(about = "Which state of a workspace to read: by default main's (its directory as it is now), else a changeset or snapshot")]
pub struct FsAt {
    pub ws: String,
    #[schemars(description = "A changeset or snapshot of the workspace.")]
    #[arg(help = "A changeset or snapshot of the workspace")]
    #[serde(default)]
    #[arg(long)]
    pub at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`fs.read`: a file's content (from `offset`, at most `length` bytes) or a symlink's target.")]
pub struct FsRead {
    #[serde(flatten)]
    pub at: FsAt,
    pub path: PathName,
    #[serde(default)]
    pub offset: Option<u64>,
    #[schemars(description = "At most this many bytes (default: to the end, up to 64 MiB).")]
    #[serde(default)]
    pub length: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`fs.list`: the entries of a directory.")]
pub struct FsList {
    #[serde(flatten)]
    pub at: FsAt,
    #[schemars(description = "Directory; default the workspace root.")]
    #[serde(default)]
    pub path: Option<PathName>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`fs.stat`: one entry.")]
pub struct FsStat {
    #[serde(flatten)]
    pub at: FsAt,
    pub path: PathName,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "lowercase")]
#[schemars(description = "One change of `fs.edit`. New entries belong to the workspace owner.")]
pub enum FsOp {
    #[schemars(description = "Create or replace a regular file; missing parent directories are created.")]
    Write {
        path: PathName,
        #[schemars(description = "Base64.")]
        content: String,
        #[schemars(description = "Default: the replaced file's mode, else 0644.")]
        #[serde(default)]
        mode: Option<u32>,
    },
    #[schemars(description = "Create a directory and missing parents.")]
    Mkdir {
        path: PathName,
        #[serde(default)]
        mode: Option<u32>,
    },
    #[schemars(description = "Remove an entry, with everything below it.")]
    Delete {
        path: PathName,
    },
    #[schemars(description = "Rename an entry; `to` must not exist.")]
    Rename {
        from: PathName,
        to: PathName,
    },
    Symlink {
        path: PathName,
        target: PathName,
    },
    Chmod {
        path: PathName,
        mode: u32,
    },
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct FsEdit {
    pub ws: String,
    #[schemars(description = "The changeset or snapshot the changes were made against: refuse them if a path\nthey touch changed since.")]
    #[serde(default)]
    pub base: Option<String>,
    pub ops: Vec<FsOp>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub tool_call: Option<String>,
}
