/* Generated from schema/acfs.schema.json by sdk/gen.sh; do not edit. */

/**
 * A path in API messages: a string, or `{"base64": ...}` when it is not UTF-8.
 */
export type PathName = string | Base64Path;
export type MergeStatus = "merged" | "up-to-date" | "conflict" | "discarded";
/**
 * `{"op": "<name>", "args": {...}}`.
 */
export type Request =
  | PingRequest
  | WsRegisterRequest
  | WsListRequest
  | WsDetachRequest
  | WsPruneRequest
  | ChangesetRequest
  | ResetRequest
  | RevertRequest
  | BasePromoteRequest
  | BaseFlattenRequest
  | SessionCreateRequest
  | SessionListRequest
  | SessionMergeRequest
  | SessionDiscardRequest
  | ProcRunRequest
  | ProcChangesetRequest
  | ProcMergeRequest
  | ProcKillRequest
  | ProcShowRequest
  | ProcListRequest
  | LogRequest
  | DiffRequest
  | PushRequest
  | PullRequest
  | FsReadRequest
  | FsListRequest
  | FsStatRequest
  | FsEditRequest;
export type ResolutionAction = "ours" | "theirs" | "content";
/**
 * One change of `fs.edit`. New entries belong to the workspace owner.
 */
export type FsOp =
  | {
      /**
       * Base64.
       */
      content: string;
      /**
       * Default: the replaced file's mode, else 0644.
       */
      mode?: number | null;
      op: "write";
      path: PathName;
    }
  | {
      mode?: number | null;
      op: "mkdir";
      path: PathName;
    }
  | {
      op: "delete";
      path: PathName;
    }
  | {
      from: PathName;
      op: "rename";
      to: PathName;
    }
  | {
      op: "symlink";
      path: PathName;
      target: PathName;
    }
  | {
      mode: number;
      op: "chmod";
      path: PathName;
    };
export type ChangeKind = "add" | "modify" | "delete" | "rename";
export type FsEditStatus = "applied" | "unchanged" | "conflict";
export type FsKind = "file" | "dir" | "symlink" | "fifo" | "other";
export type PullStatus = "up-to-date" | "fast-forward" | "created";

/**
 * acfs-daemon socket protocol: one JSON request line and one JSON response line per connection
 * (`proc.run`: a second line, `ProcExited`).
 */
export interface AcfsProtocol {
  proc_exited: ProcExited;
  protocol: 2;
  request: Request;
  response: Response;
  results: {
    "base.flatten": BaseChanged;
    "base.promote": BaseChanged;
    changeset: LiveChangeset;
    diff: DiffResult[];
    "fs.edit": FsEdited;
    "fs.list": FsListing;
    "fs.read": FsFile;
    "fs.stat": FsEntry;
    log: ChangesetInfo[];
    ping: Pong;
    "proc.changeset": ProcChangesetTaken;
    "proc.kill": ProcKilled;
    "proc.list": ProcInfo[];
    "proc.merge": ProcMerged;
    "proc.run": ProcStarted;
    "proc.show": ProcInfo;
    pull: Pulled;
    push: Pushed;
    reset: Applied;
    revert: Applied;
    "session.create": SessionCreated;
    "session.discard": SessionEnded;
    "session.list": SessionInfo[];
    "session.merge": SessionEnded;
    "ws.detach": WsDetached;
    "ws.list": WsInfo[];
    "ws.prune": Pruned;
    "ws.register": WsRegistered;
  };
}
/**
 * `proc.run` (second response line): the process exited and its work was merged.
 */
export interface ProcExited {
  /**
   * Exit code; None when killed by a signal (or unknown after a daemon restart).
   */
  code?: number | null;
  id: string;
  merge: ProcMerged;
  signal?: number | null;
}
/**
 * `proc.merge`.
 */
export interface ProcMerged {
  /**
   * Snapshot of the process's branches taken before merging.
   */
  changeset?: string | null;
  conflict: boolean;
  id: string;
  /**
   * What the resolver did, when it ran.
   */
  resolver?: string | null;
  /**
   * Merges into main, one per workspace with changes.
   */
  results: MergeResult[];
  /**
   * The process's state afterwards: running, conflicted, merged or discarded.
   */
  state: string;
  /**
   * Running process: main's other changes merged into its branches.
   */
  updated: MergeResult[];
}
/**
 * Merge into one workspace's branch (proc.merge, reset, revert, session.discard).
 */
export interface MergeResult {
  /**
   * Entries written (merged).
   */
  actions?: number | null;
  /**
   * Changeset recording the merge (merged).
   */
  changeset?: string | null;
  conflicts?: ConflictInfo[];
  /**
   * The snapshot merged in.
   */
  from?: string | null;
  snap?: string | null;
  status: MergeStatus;
  /**
   * Resolutions given for paths that had no conflict.
   */
  unused_resolutions?: PathName[];
  ws: string;
}
export interface ConflictInfo {
  path: PathName;
  reason: string;
}
export interface Base64Path {
  base64: string;
}
export interface PingRequest {
  op: "ping";
}
/**
 * Take over a directory: acfs mounts the workspace's main over it and records every change made there.
 */
export interface WsRegisterRequest {
  args: WsRegisterArgs;
  op: "ws.register";
}
export interface WsRegisterArgs {
  from: string;
  name: string;
}
export interface WsListRequest {
  op: "ws.list";
}
/**
 * Stop tracking: write the head back to the host directory and unmount.
 */
export interface WsDetachRequest {
  args: WsDetachArgs;
  op: "ws.detach";
}
export interface WsDetachArgs {
  ws: string;
}
/**
 * Thin out a workspace's automatic snapshots now (also done periodically).
 */
export interface WsPruneRequest {
  args: WsPrune;
  op: "ws.prune";
}
export interface WsPrune {
  /**
   * Only report what would be dropped.
   */
  dry_run?: boolean;
  ws: string;
}
export interface ChangesetRequest {
  args: Changeset;
  op: "changeset";
}
/**
 * Snapshot workspaces now (their main: the directory as it is).
 */
export interface Changeset {
  author?: string | null;
  message?: string | null;
  session?: string | null;
  tool_call?: string | null;
  /**
   * Workspaces; default: those of `--session`.
   */
  ws?: string[];
}
export interface ResetRequest {
  args: Reset;
  op: "reset";
}
/**
 * Bring a branch back to a changeset or snapshot (recorded as a new changeset): a running
 * process's branch for its own snapshots, else the workspace's main.
 */
export interface Reset {
  /**
   * Changeset or snapshot id.
   */
  target: string;
}
export interface RevertRequest {
  args: Revert;
  op: "revert";
}
/**
 * Undo one changeset, everything a session recorded, or everything between two states
 * (`from`, `to`), on top of the current head.
 */
export interface Revert {
  changeset?: string | null;
  /**
   * Changeset or snapshot the range starts at (not undone); needs `to`.
   */
  from?: string | null;
  resolutions?: ResolutionSpec[];
  session?: string | null;
  /**
   * Changeset or snapshot the range ends at (undone), of the same workspaces as `from`.
   */
  to?: string | null;
}
/**
 * Conflict resolution for one path.
 */
export interface ResolutionSpec {
  action: ResolutionAction;
  /**
   * Base64 file content for action "content" (the CLI reads `content_path`).
   */
  content?: string | null;
  path: PathName;
  ws: string;
}
export interface BasePromoteRequest {
  args: BasePromoteArgs;
  op: "base.promote";
}
export interface BasePromoteArgs {
  snap: string;
}
export interface BaseFlattenRequest {
  args: BaseFlattenArgs;
  op: "base.flatten";
}
export interface BaseFlattenArgs {
  ws: string;
}
export interface SessionCreateRequest {
  args: SessionCreate;
  op: "session.create";
}
export interface SessionCreate {
  /**
   * End (merge) the session once the calling process (the one that connected) exits,
   * as if its idle timeout had passed. Needs Linux 6.5 (SO_PEERPIDFD). SDK only: the
   * CLI process exits right away.
   */
  end_on_exit?: boolean;
  /**
   * End (merge) the session after this many seconds without changes.
   */
  idle_timeout?: number | null;
  name?: string | null;
  /**
   * Workspaces whose changes this session labels (repeatable).
   */
  ws?: string[];
}
export interface SessionListRequest {
  op: "session.list";
}
/**
 * End a session: its workspaces keep their content, the label stops.
 */
export interface SessionMergeRequest {
  args: SessionMergeArgs;
  op: "session.merge";
}
export interface SessionMergeArgs {
  session: string;
}
/**
 * End a session and bring its workspaces back to where it started.
 */
export interface SessionDiscardRequest {
  args: SessionDiscardArgs;
  op: "session.discard";
}
export interface SessionDiscardArgs {
  session: string;
}
/**
 * Run a command as the caller in its own mount namespace, every workspace replaced by
 * the process's own branch. CLI only: the request carries the caller's stdin, stdout and
 * stderr (SCM_RIGHTS); a second response line (`ProcExited`) follows when it exits.
 */
export interface ProcRunRequest {
  args: ProcRun;
  op: "proc.run";
}
export interface ProcRun {
  cmd: string[];
  /**
   * Working directory (inside the process's view).
   */
  cwd: string;
  /**
   * The whole environment, `KEY=value`.
   */
  env?: string[];
  /**
   * Command run as the caller, in a directory with `TASK.md`, when merging conflicts.
   */
  resolver?: string[];
  /**
   * Workspaces the process gets a branch of; default: every attached workspace of the caller.
   */
  ws?: string[];
}
export interface ProcChangesetRequest {
  args: ProcChangeset;
  op: "proc.changeset";
}
export interface ProcChangeset {
  author: string;
  message: string;
  proc_id: string;
  /**
   * Record nothing when every branch still equals this changeset's snapshot of its workspace (default: the branch's head).
   */
  since?: string | null;
  tool_call?: string | null;
}
/**
 * The process's task is done: snapshot its branches, merge them into main (the resolver
 * handles conflicts) and, while it runs, bring main's other changes into its branches.
 */
export interface ProcMergeRequest {
  args: ProcMerge;
  op: "proc.merge";
}
export interface ProcMerge {
  /**
   * Workspaces whose unmerged changes are dropped (the branch is reset to main).
   */
  discard_ws?: string[];
  /**
   * Report conflicts instead of running the resolver.
   */
  no_resolver?: boolean;
  proc_id: string;
  resolutions?: ResolutionSpec[];
}
/**
 * Signal a running process's group (default SIGTERM).
 */
export interface ProcKillRequest {
  args: ProcKillArgs;
  op: "proc.kill";
}
export interface ProcKillArgs {
  proc_id: string;
  signal?: number | null;
}
export interface ProcShowRequest {
  args: ProcShowArgs;
  op: "proc.show";
}
export interface ProcShowArgs {
  proc_id: string;
}
export interface ProcListRequest {
  op: "proc.list";
}
export interface LogRequest {
  args: Log;
  op: "log";
}
export interface Log {
  n?: number | null;
  proc_id?: string | null;
  session?: string | null;
  ws?: string | null;
}
export interface DiffRequest {
  args: Diff;
  op: "diff";
}
export interface Diff {
  a: string;
  /**
   * Also list paths that `.acfsignore` leaves out.
   */
  all?: boolean;
  b?: string | null;
  /**
   * Compare complete views instead of the upper-layer shortcut (slow; for verification).
   */
  full?: boolean;
  patch?: boolean;
}
export interface PushRequest {
  args: Remote;
  op: "push";
}
export interface Remote {
  remote?: string | null;
  ws: string;
}
export interface PullRequest {
  args: Remote;
  op: "pull";
}
/**
 * Read a file (or a symlink's target) of a workspace without mounting it.
 */
export interface FsReadRequest {
  args: FsRead;
  op: "fs.read";
}
/**
 * `fs.read`: a file's content (from `offset`, at most `length` bytes) or a symlink's target.
 */
export interface FsRead {
  /**
   * A changeset or snapshot of the workspace.
   */
  at?: string | null;
  /**
   * At most this many bytes (default: to the end, up to 64 MiB).
   */
  length?: number | null;
  offset?: number | null;
  path: PathName;
  ws: string;
}
/**
 * List a directory of a workspace.
 */
export interface FsListRequest {
  args: FsList;
  op: "fs.list";
}
/**
 * `fs.list`: the entries of a directory.
 */
export interface FsList {
  /**
   * A changeset or snapshot of the workspace.
   */
  at?: string | null;
  /**
   * Directory; default the workspace root.
   */
  path?: PathName | null;
  ws: string;
}
export interface FsStatRequest {
  args: FsStat;
  op: "fs.stat";
}
/**
 * `fs.stat`: one entry.
 */
export interface FsStat {
  /**
   * A changeset or snapshot of the workspace.
   */
  at?: string | null;
  path: PathName;
  ws: string;
}
/**
 * Change files of a workspace's main and record the change as one changeset.
 */
export interface FsEditRequest {
  args: FsEdit;
  op: "fs.edit";
}
export interface FsEdit {
  author?: string | null;
  /**
   * The changeset or snapshot the changes were made against: refuse them if a path
   * they touch changed since.
   */
  base?: string | null;
  message?: string | null;
  ops: FsOp[];
  tool_call?: string | null;
  ws: string;
}
export interface Response {
  error?: ErrorBody | null;
  ok: boolean;
  result?: unknown;
}
export interface ErrorBody {
  /**
   * invalid, not_found, state (not allowed in the current state), exists, exhausted or internal.
   */
  code: string;
  message: string;
}
/**
 * `base.promote` and `base.flatten`.
 */
export interface BaseChanged {
  base: string;
  changeset: string;
  head_snap: string;
  layers: number;
  ws: string;
}
/**
 * `changeset`.
 */
export interface LiveChangeset {
  /**
   * None when nothing changed since the last changeset.
   */
  changeset?: string | null;
  session?: string | null;
  unchanged: boolean;
}
/**
 * An entry of `diff`: one workspace.
 */
export interface DiffResult {
  changes: Change[];
  from?: string | null;
  /**
   * Unified diff of regular files, with `patch`.
   */
  patch?: string | null;
  /**
   * The snapshot was pruned: `changes` is the summary kept against its parent.
   */
  summary_only?: boolean;
  to: string;
  ws: string;
}
export interface Change {
  /**
   * Source of a renamed directory.
   */
  from?: PathName | null;
  kind: ChangeKind;
  path: PathName;
}
/**
 * `fs.edit`.
 */
export interface FsEdited {
  changeset?: string | null;
  /**
   * Same as `status == conflict` (the CLI exits 2).
   */
  conflict: boolean;
  /**
   * With `conflict`: paths changed since `base`.
   */
  conflicts?: PathName[];
  snap?: string | null;
  status: FsEditStatus;
  ws: string;
}
/**
 * `fs.list`.
 */
export interface FsListing {
  entries: FsEntry[];
  snap?: string | null;
  ws: string;
}
/**
 * `fs.stat`, and entries of `fs.list`.
 */
export interface FsEntry {
  gid: number;
  kind: FsKind;
  mode: number;
  /**
   * Unix time in ms.
   */
  mtime: number;
  /**
   * A path in API messages: a string, or `{"base64": ...}` when it is not UTF-8.
   */
  path: string | Base64Path;
  size: number;
  uid: number;
}
/**
 * `fs.read`.
 */
export interface FsFile {
  /**
   * Base64 content of a regular file (from `offset`).
   */
  content?: string | null;
  entry: FsEntry;
  /**
   * Snapshot read; None for the workspace's directory.
   */
  snap?: string | null;
  /**
   * Target of a symlink.
   */
  target?: PathName | null;
  ws: string;
}
/**
 * An entry of `log`.
 */
export interface ChangesetInfo {
  author: string;
  /**
   * Unix time in ms.
   */
  created_at: number;
  id: string;
  message: string;
  owner: string;
  /**
   * workspace or process.
   */
  owner_kind: string;
  /**
   * Session labelling the workspace when the changeset was taken.
   */
  session?: string | null;
  snaps: SnapInfo[];
  tool_call?: string | null;
}
export interface SnapInfo {
  branch: string;
  merge_parent?: string | null;
  parent?: string | null;
  /**
   * False once pruned (only the diff summary against the parent is kept).
   */
  present: boolean;
  snap: string;
  ws: string;
}
/**
 * `ping`.
 */
export interface Pong {
  /**
   * [`crate::proto::PROTOCOL`] of the daemon.
   */
  protocol: number;
  /**
   * Daemon version.
   */
  version: string;
}
/**
 * `proc.changeset`.
 */
export interface ProcChangesetTaken {
  /**
   * None when nothing changed (see `since`); no snapshot is kept then.
   */
  changeset?: string | null;
  pause_ms: number;
  /**
   * Whether the process was frozen (it was running).
   */
  paused: boolean;
  snaps: SnapRef[];
  unchanged: boolean;
}
export interface SnapRef {
  branch: string;
  snap: string;
  ws: string;
}
/**
 * `proc.kill`.
 */
export interface ProcKilled {
  id: string;
  /**
   * False when the process was not running.
   */
  sent: boolean;
  signal: number;
}
/**
 * `proc.show` and entries of `proc.list`.
 */
export interface ProcInfo {
  branches: ProcBranch[];
  cmd: string[];
  /**
   * Unix time in ms.
   */
  created_at: number;
  cwd: string;
  id: string;
  pid?: number | null;
  /**
   * running, exiting (exited, its work is being merged), conflicted (exited, work not merged yet), merged
   * or discarded.
   */
  state: string;
}
export interface ProcBranch {
  branch: string;
  head_snap: string;
  /**
   * The workspace's directory; the process sees its branch there.
   */
  path: string;
  /**
   * open (work not merged yet), conflicted, merged or discarded.
   */
  state: string;
  ws: string;
}
/**
 * `proc.run` (first response line).
 */
export interface ProcStarted {
  branches: ProcBranch[];
  id: string;
  pid: number;
}
/**
 * `pull`.
 */
export interface Pulled {
  bases_received: number;
  head: string;
  snaps_received: number;
  status: PullStatus;
  ws: string;
}
/**
 * `push`.
 */
export interface Pushed {
  bases_sent: number;
  bytes: number;
  head: string;
  remote: string;
  snaps_sent: number;
  ws: string;
}
/**
 * `reset` and `revert`.
 */
export interface Applied {
  conflict: boolean;
  results: MergeResult[];
}
/**
 * `session.create`.
 */
export interface SessionCreated {
  id: string;
  live: LiveSpan[];
  name?: string | null;
}
/**
 * A stretch of a workspace's history labelled by a session.
 */
export interface LiveSpan {
  /**
   * `session.discard`: bringing the workspace back to `start_snap`.
   */
  discard?: MergeResult | null;
  /**
   * None while the session is active.
   */
  end_snap?: string | null;
  start_snap: string;
  ws: string;
}
/**
 * `session.merge` and `session.discard`.
 */
export interface SessionEnded {
  /**
   * `session.discard`: a workspace could not be brought back (see `live[].discard`).
   */
  conflict: boolean;
  live: LiveSpan[];
  session: string;
  /**
   * merged or discarded.
   */
  state: string;
}
/**
 * An entry of `session.list`.
 */
export interface SessionInfo {
  /**
   * Unix time in ms.
   */
  created_at: number;
  id: string;
  live: LiveSpan[];
  name?: string | null;
  /**
   * active, merged or discarded.
   */
  state: string;
}
/**
 * `ws.detach`.
 */
export interface WsDetached {
  head_snap: string;
  ws: string;
}
/**
 * An entry of `ws.list`.
 */
export interface WsInfo {
  base: string;
  gid: number;
  head_snap: string;
  host_path: string;
  id: string;
  layers: number;
  /**
   * False once detached.
   */
  live: boolean;
  main_branch: string;
  name: string;
  /**
   * Changes not recorded yet.
   */
  pending: boolean;
  /**
   * Active session labelling this workspace.
   */
  session?: string | null;
  uid: number;
}
/**
 * `ws.prune`.
 */
export interface Pruned {
  /**
   * Snapshots whose data was dropped (they keep metadata and a diff summary).
   */
  dropped: string[];
  dry_run: boolean;
  ws: string;
}
/**
 * `ws.register`.
 */
export interface WsRegistered {
  base: string;
  changeset: string;
  gid: number;
  head_snap: string;
  host_path: string;
  id: string;
  main_branch: string;
  name: string;
  /**
   * Processes that were inside the directory when acfs took it over; they see it vanish.
   */
  stale_processes: string[];
  uid: number;
}
