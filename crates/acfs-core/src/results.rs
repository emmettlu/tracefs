use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::merge::ChangeKind;
use crate::path::PathName;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`ping`.")]
pub struct Pong {
    #[schemars(description = "Daemon version.")]
    pub version: String,
    #[schemars(description = "[`crate::proto::PROTOCOL`] of the daemon.")]
    pub protocol: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`ws.register`.")]
pub struct WsRegistered {
    pub id: String,
    pub name: String,
    pub host_path: String,
    pub uid: u32,
    pub gid: u32,
    pub base: String,
    pub main_branch: String,
    pub head_snap: String,
    pub changeset: String,
    #[schemars(description = "Processes that were inside the directory when acfs took it over; they see it vanish.")]
    pub stale_processes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "An entry of `ws.list`.")]
pub struct WsInfo {
    pub id: String,
    pub name: String,
    pub host_path: String,
    pub uid: u32,
    pub gid: u32,
    pub main_branch: String,
    pub head_snap: String,
    pub base: String,
    pub layers: u32,
    #[schemars(description = "False once detached.")]
    pub live: bool,
    #[schemars(description = "Active session labelling this workspace.")]
    pub session: Option<String>,
    #[schemars(description = "Changes not recorded yet.")]
    pub pending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`ws.detach`.")]
pub struct WsDetached {
    pub ws: String,
    pub head_snap: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`changeset`.")]
pub struct LiveChangeset {
    #[schemars(description = "None when nothing changed since the last changeset.")]
    pub changeset: Option<String>,
    pub unchanged: bool,
    pub session: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`reset` and `revert`.")]
pub struct Applied {
    pub results: Vec<MergeResult>,
    pub conflict: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`base.promote` and `base.flatten`.")]
pub struct BaseChanged {
    pub ws: String,
    pub base: String,
    pub layers: u32,
    pub head_snap: String,
    pub changeset: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`session.create`.")]
pub struct SessionCreated {
    pub id: String,
    pub name: Option<String>,
    pub live: Vec<LiveSpan>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "An entry of `session.list`.")]
pub struct SessionInfo {
    pub id: String,
    pub name: Option<String>,
    #[schemars(description = "active, merged or discarded.")]
    pub state: String,
    #[schemars(description = "Unix time in ms.")]
    pub created_at: i64,
    pub live: Vec<LiveSpan>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "A stretch of a workspace's history labelled by a session.")]
pub struct LiveSpan {
    pub ws: String,
    pub start_snap: String,
    #[schemars(description = "None while the session is active.")]
    pub end_snap: Option<String>,
    #[schemars(description = "`session.discard`: bringing the workspace back to `start_snap`.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discard: Option<MergeResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`session.merge` and `session.discard`.")]
pub struct SessionEnded {
    pub session: String,
    #[schemars(description = "merged or discarded.")]
    pub state: String,
    pub live: Vec<LiveSpan>,
    #[schemars(description = "`session.discard`: a workspace could not be brought back (see `live[].discard`).")]
    pub conflict: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProcBranch {
    pub ws: String,
    #[schemars(description = "The workspace's directory; the process sees its branch there.")]
    pub path: String,
    pub branch: String,
    pub head_snap: String,
    #[schemars(description = "open (work not merged yet), conflicted, merged or discarded.")]
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`proc.run` (first response line).")]
pub struct ProcStarted {
    pub id: String,
    pub pid: i32,
    pub branches: Vec<ProcBranch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`proc.run` (second response line): the process exited and its work was merged.")]
pub struct ProcExited {
    pub id: String,
    #[schemars(description = "Exit code; None when killed by a signal (or unknown after a daemon restart).")]
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub merge: ProcMerged,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`proc.show` and entries of `proc.list`.")]
pub struct ProcInfo {
    pub id: String,
    #[schemars(description = "running, exiting (exited, its work is being merged), conflicted (exited, work not merged yet), merged\nor discarded.")]
    pub state: String,
    pub pid: Option<i32>,
    pub cmd: Vec<String>,
    pub cwd: String,
    #[schemars(description = "Unix time in ms.")]
    pub created_at: i64,
    pub branches: Vec<ProcBranch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`proc.changeset`.")]
pub struct ProcChangesetTaken {
    #[schemars(description = "None when nothing changed (see `since`); no snapshot is kept then.")]
    pub changeset: Option<String>,
    pub unchanged: bool,
    pub snaps: Vec<SnapRef>,
    #[schemars(description = "Whether the process was frozen (it was running).")]
    pub paused: bool,
    pub pause_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnapRef {
    pub ws: String,
    pub snap: String,
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`proc.merge`.")]
pub struct ProcMerged {
    pub id: String,
    #[schemars(description = "The process's state afterwards: running, conflicted, merged or discarded.")]
    pub state: String,
    #[schemars(description = "Snapshot of the process's branches taken before merging.")]
    pub changeset: Option<String>,
    #[schemars(description = "Merges into main, one per workspace with changes.")]
    pub results: Vec<MergeResult>,
    #[schemars(description = "Running process: main's other changes merged into its branches.")]
    pub updated: Vec<MergeResult>,
    pub conflict: bool,
    #[schemars(description = "What the resolver did, when it ran.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolver: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "An entry of `log`.")]
pub struct ChangesetInfo {
    pub id: String,
    #[schemars(description = "workspace or process.")]
    pub owner_kind: String,
    pub owner: String,
    pub author: String,
    pub message: String,
    pub tool_call: Option<String>,
    #[schemars(description = "Unix time in ms.")]
    pub created_at: i64,
    #[schemars(description = "Session labelling the workspace when the changeset was taken.")]
    pub session: Option<String>,
    pub snaps: Vec<SnapInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnapInfo {
    pub snap: String,
    pub ws: String,
    pub branch: String,
    pub parent: Option<String>,
    pub merge_parent: Option<String>,
    #[schemars(description = "False once pruned (only the diff summary against the parent is kept).")]
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "An entry of `diff`: one workspace.")]
pub struct DiffResult {
    pub ws: String,
    pub from: Option<String>,
    pub to: String,
    pub changes: Vec<Change>,
    #[schemars(description = "The snapshot was pruned: `changes` is the summary kept against its parent.")]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub summary_only: bool,
    #[schemars(description = "Unified diff of regular files, with `patch`.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Change {
    pub path: PathName,
    pub kind: ChangeKind,
    #[schemars(description = "Source of a renamed directory.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<PathName>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`push`.")]
pub struct Pushed {
    pub ws: String,
    pub remote: String,
    pub bases_sent: usize,
    pub snaps_sent: usize,
    pub bytes: u64,
    pub head: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PullStatus {
    UpToDate,
    FastForward,
    Created,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`pull`.")]
pub struct Pulled {
    pub ws: String,
    pub status: PullStatus,
    pub bases_received: usize,
    pub snaps_received: usize,
    pub head: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum MergeStatus {
    Merged,
    UpToDate,
    Conflict,
    Discarded,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Merge into one workspace's branch (proc.merge, reset, revert, session.discard).")]
pub struct MergeResult {
    pub ws: String,
    pub status: MergeStatus,
    #[schemars(description = "Entries written (merged).")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<usize>,
    #[schemars(description = "Changeset recording the merge (merged).")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changeset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snap: Option<String>,
    #[schemars(description = "The snapshot merged in.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default)]
    pub conflicts: Vec<ConflictInfo>,
    #[schemars(description = "Resolutions given for paths that had no conflict.")]
    #[serde(default)]
    pub unused_resolutions: Vec<PathName>,
}

impl MergeResult {
    pub fn new(ws: impl Into<String>, status: MergeStatus) -> Self {
        MergeResult { ws: ws.into(), status, actions: None, changeset: None, snap: None, from: None, conflicts: Vec::new(), unused_resolutions: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConflictInfo {
    pub path: PathName,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`ws.prune`.")]
pub struct Pruned {
    pub ws: String,
    #[schemars(description = "Snapshots whose data was dropped (they keep metadata and a diff summary).")]
    pub dropped: Vec<String>,
    pub dry_run: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FsKind {
    File,
    Dir,
    Symlink,
    Fifo,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`fs.stat`, and entries of `fs.list`.")]
pub struct FsEntry {
    #[schemars(description = "Path from the workspace root (`fs.stat`) or name in the directory (`fs.list`).")]
    pub path: PathName,
    pub kind: FsKind,
    pub size: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    #[schemars(description = "Unix time in ms.")]
    pub mtime: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`fs.read`.")]
pub struct FsFile {
    pub ws: String,
    #[schemars(description = "Snapshot read; None for the workspace's directory.")]
    pub snap: Option<String>,
    pub entry: FsEntry,
    #[schemars(description = "Base64 content of a regular file (from `offset`).")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[schemars(description = "Target of a symlink.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<PathName>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`fs.list`.")]
pub struct FsListing {
    pub ws: String,
    pub snap: Option<String>,
    pub entries: Vec<FsEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FsEditStatus {
    Applied,
    #[schemars(description = "The changes left the workspace as it was.")]
    Unchanged,
    #[schemars(description = "Paths the changes touch changed since `base`; nothing was written.")]
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`fs.edit`.")]
pub struct FsEdited {
    pub ws: String,
    pub status: FsEditStatus,
    pub changeset: Option<String>,
    pub snap: Option<String>,
    #[schemars(description = "With `conflict`: paths changed since `base`.")]
    #[serde(default)]
    pub conflicts: Vec<PathName>,
    #[schemars(description = "Same as `status == conflict` (the CLI exits 2).")]
    pub conflict: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "`proc.kill`.")]
pub struct ProcKilled {
    pub id: String,
    pub signal: i32,
    #[schemars(description = "False when the process was not running.")]
    pub sent: bool,
}
