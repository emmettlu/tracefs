# Generated from schema/tracefs.schema.json by sdk/gen.sh; do not edit.

from __future__ import annotations

from typing import Any, Literal, NotRequired, TypeAlias, TypedDict


class BaseChanged(TypedDict):
    """
    `base.promote` and `base.flatten`.
    """

    base: str
    changeset: str
    head_snap: str
    layers: int
    ws: str


ChangeKind: TypeAlias = Literal['add', 'modify', 'delete', 'rename']


class Changeset(TypedDict):
    """
    Snapshot workspaces now (their main: the directory as it is).
    """

    author: NotRequired[str | None]
    message: NotRequired[str | None]
    session: NotRequired[str | None]
    tool_call: NotRequired[str | None]
    ws: NotRequired[list[str]]
    """
    Workspaces; default: those of `--session`.
    """


class Diff(TypedDict):
    a: str
    all: NotRequired[bool]
    """
    Also list paths that `.tracefsignore` leaves out.
    """
    b: NotRequired[str | None]
    full: NotRequired[bool]
    """
    Compare complete views instead of the upper-layer shortcut (slow; for verification).
    """
    patch: NotRequired[bool]


class ErrorBody(TypedDict):
    code: str
    """
    invalid, not_found, state (not allowed in the current state), exists, exhausted or internal.
    """
    message: str


FsEditStatus: TypeAlias = Literal['applied'] | Literal['unchanged'] | Literal['conflict']


FsKind: TypeAlias = Literal['file', 'dir', 'symlink', 'fifo', 'other']


class LiveChangeset(TypedDict):
    """
    `changeset`.
    """

    changeset: NotRequired[str | None]
    """
    None when nothing changed since the last changeset.
    """
    session: NotRequired[str | None]
    unchanged: bool


class Log(TypedDict):
    n: NotRequired[int | None]
    proc_id: NotRequired[str | None]
    session: NotRequired[str | None]
    ws: NotRequired[str | None]


MergeStatus: TypeAlias = Literal['merged', 'up-to-date', 'conflict', 'discarded']


class Base64Path(TypedDict):
    """
    A path in API messages: a string, or `{"base64": ...}` when it is not UTF-8.
    """

    base64: str


PathName: TypeAlias = str | Base64Path
"""
A path in API messages: a string, or `{"base64": ...}` when it is not UTF-8.
"""


class Pong(TypedDict):
    """
    `ping`.
    """

    protocol: int
    """
    [`crate::proto::PROTOCOL`] of the daemon.
    """
    version: str
    """
    Daemon version.
    """


class ProcBranch(TypedDict):
    branch: str
    head_snap: str
    path: str
    """
    The workspace's directory; the process sees its branch there.
    """
    state: str
    """
    open (work not merged yet), conflicted, merged or discarded.
    """
    ws: str


class ProcChangeset(TypedDict):
    author: str
    message: str
    proc_id: str
    since: NotRequired[str | None]
    """
    Record nothing when every branch still equals this changeset's snapshot of its workspace (default: the branch's head).
    """
    tool_call: NotRequired[str | None]


class ProcInfo(TypedDict):
    """
    `proc.show` and entries of `proc.list`.
    """

    branches: list[ProcBranch]
    cmd: list[str]
    created_at: int
    """
    Unix time in ms.
    """
    cwd: str
    id: str
    pid: NotRequired[int | None]
    state: str
    """
    running, exiting (exited, its work is being merged), conflicted (exited, work not merged yet), merged
    or discarded.
    """


class ProcKilled(TypedDict):
    """
    `proc.kill`.
    """

    id: str
    sent: bool
    """
    False when the process was not running.
    """
    signal: int


class ProcRun(TypedDict):
    cmd: list[str]
    cwd: str
    """
    Working directory (inside the process's view).
    """
    env: NotRequired[list[str]]
    """
    The whole environment, `KEY=value`.
    """
    resolver: NotRequired[list[str]]
    """
    Command run as the caller, in a directory with `TASK.md`, when merging conflicts.
    """
    ws: NotRequired[list[str]]
    """
    Workspaces the process gets a branch of; default: every attached workspace of the caller.
    """


class ProcStarted(TypedDict):
    """
    `proc.run` (first response line).
    """

    branches: list[ProcBranch]
    id: str
    pid: int


class Pruned(TypedDict):
    """
    `ws.prune`.
    """

    dropped: list[str]
    """
    Snapshots whose data was dropped (they keep metadata and a diff summary).
    """
    dry_run: bool
    ws: str


PullStatus: TypeAlias = Literal['up-to-date', 'fast-forward', 'created']


class Pulled(TypedDict):
    """
    `pull`.
    """

    bases_received: int
    head: str
    snaps_received: int
    status: PullStatus
    ws: str


class Pushed(TypedDict):
    """
    `push`.
    """

    bases_sent: int
    bytes: int
    head: str
    remote: str
    snaps_sent: int
    ws: str


class Remote(TypedDict):
    remote: NotRequired[str | None]
    ws: str


class PingRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    op: Literal['ping']


WsRegisterArgs = TypedDict(
    'WsRegisterArgs',
    {
        'from': str,
        'name': str,
    },
)


class WsRegisterRequest(TypedDict):
    """
    Take over a directory: tracefs mounts the workspace's main over it and records every change made there.
    """

    args: WsRegisterArgs
    op: Literal['ws.register']


class WsListRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    op: Literal['ws.list']


class WsDetachArgs(TypedDict):
    ws: str


class WsDetachRequest(TypedDict):
    """
    Stop tracking: write the head back to the host directory and unmount.
    """

    args: WsDetachArgs
    op: Literal['ws.detach']


class ChangesetRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    args: Changeset
    op: Literal['changeset']


class BasePromoteArgs(TypedDict):
    snap: str


class BasePromoteRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    args: BasePromoteArgs
    op: Literal['base.promote']


class BaseFlattenArgs(TypedDict):
    ws: str


class BaseFlattenRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    args: BaseFlattenArgs
    op: Literal['base.flatten']


class SessionListRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    op: Literal['session.list']


class SessionMergeArgs(TypedDict):
    session: str


class SessionMergeRequest(TypedDict):
    """
    End a session: its workspaces keep their content, the label stops.
    """

    args: SessionMergeArgs
    op: Literal['session.merge']


class SessionDiscardArgs(TypedDict):
    session: str


class SessionDiscardRequest(TypedDict):
    """
    End a session and bring its workspaces back to where it started.
    """

    args: SessionDiscardArgs
    op: Literal['session.discard']


class ProcRunRequest(TypedDict):
    """
    Run a command as the caller in its own mount namespace, every workspace replaced by
    the process's own branch. CLI only: the request carries the caller's stdin, stdout and
    stderr (SCM_RIGHTS); a second response line (`ProcExited`) follows when it exits.
    """

    args: ProcRun
    op: Literal['proc.run']


class ProcChangesetRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    args: ProcChangeset
    op: Literal['proc.changeset']


class ProcKillArgs(TypedDict):
    proc_id: str
    signal: NotRequired[int | None]


class ProcKillRequest(TypedDict):
    """
    Signal a running process's group (default SIGTERM).
    """

    args: ProcKillArgs
    op: Literal['proc.kill']


class ProcShowArgs(TypedDict):
    proc_id: str


class ProcShowRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    args: ProcShowArgs
    op: Literal['proc.show']


class ProcListRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    op: Literal['proc.list']


class LogRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    args: Log
    op: Literal['log']


class DiffRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    args: Diff
    op: Literal['diff']


class PushRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    args: Remote
    op: Literal['push']


class PullRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    args: Remote
    op: Literal['pull']


class Reset(TypedDict):
    """
    Bring a branch back to a changeset or snapshot (recorded as a new changeset): a running
    process's branch for its own snapshots, else the workspace's main.
    """

    target: str
    """
    Changeset or snapshot id.
    """


ResolutionAction: TypeAlias = Literal['ours', 'theirs', 'content']


class ResolutionSpec(TypedDict):
    """
    Conflict resolution for one path.
    """

    action: ResolutionAction
    content: NotRequired[str | None]
    """
    Base64 file content for action "content" (the CLI reads `content_path`).
    """
    path: PathName
    ws: str


class Response(TypedDict):
    error: NotRequired[ErrorBody | None]
    ok: bool
    result: NotRequired[Any]


Revert = TypedDict(
    'Revert',
    {
        'changeset': NotRequired[str | None],
        'from': NotRequired[str | None],
        'resolutions': NotRequired[list[ResolutionSpec]],
        'session': NotRequired[str | None],
        'to': NotRequired[str | None],
    },
)


class SessionCreate(TypedDict):
    end_on_exit: NotRequired[bool]
    """
    End (merge) the session once the calling process (the one that connected) exits,
    as if its idle timeout had passed. Needs Linux 6.5 (SO_PEERPIDFD). SDK only: the
    CLI process exits right away.
    """
    idle_timeout: NotRequired[int | None]
    """
    End (merge) the session after this many seconds without changes.
    """
    name: NotRequired[str | None]
    ws: NotRequired[list[str]]
    """
    Workspaces whose changes this session labels (repeatable).
    """


class SnapInfo(TypedDict):
    branch: str
    merge_parent: NotRequired[str | None]
    parent: NotRequired[str | None]
    present: bool
    """
    False once pruned (only the diff summary against the parent is kept).
    """
    snap: str
    ws: str


class SnapRef(TypedDict):
    branch: str
    snap: str
    ws: str


class WsDetached(TypedDict):
    """
    `ws.detach`.
    """

    head_snap: str
    ws: str


class WsInfo(TypedDict):
    """
    An entry of `ws.list`.
    """

    base: str
    gid: int
    head_snap: str
    host_path: str
    id: str
    layers: int
    live: bool
    """
    False once detached.
    """
    main_branch: str
    name: str
    pending: bool
    """
    Changes not recorded yet.
    """
    session: NotRequired[str | None]
    """
    Active session labelling this workspace.
    """
    uid: int


class WsPrune(TypedDict):
    dry_run: NotRequired[bool]
    """
    Only report what would be dropped.
    """
    ws: str


class WsRegistered(TypedDict):
    """
    `ws.register`.
    """

    base: str
    changeset: str
    gid: int
    head_snap: str
    host_path: str
    id: str
    main_branch: str
    name: str
    stale_processes: list[str]
    """
    Processes that were inside the directory when tracefs took it over; they see it vanish.
    """
    uid: int


Change = TypedDict(
    'Change',
    {
        'from': NotRequired[PathName | None],
        'kind': ChangeKind,
        'path': PathName,
    },
)


class ChangesetInfo(TypedDict):
    """
    An entry of `log`.
    """

    author: str
    created_at: int
    """
    Unix time in ms.
    """
    id: str
    message: str
    owner: str
    owner_kind: str
    """
    workspace or process.
    """
    session: NotRequired[str | None]
    """
    Session labelling the workspace when the changeset was taken.
    """
    snaps: list[SnapInfo]
    tool_call: NotRequired[str | None]


class ConflictInfo(TypedDict):
    path: PathName
    reason: str


DiffResult = TypedDict(
    'DiffResult',
    {
        'changes': list[Change],
        'from': NotRequired[str | None],
        'patch': NotRequired[str | None],
        'summary_only': NotRequired[bool],
        'to': str,
        'ws': str,
    },
)


class FsEdited(TypedDict):
    """
    `fs.edit`.
    """

    changeset: NotRequired[str | None]
    conflict: bool
    """
    Same as `status == conflict` (the CLI exits 2).
    """
    conflicts: NotRequired[list[PathName]]
    """
    With `conflict`: paths changed since `base`.
    """
    snap: NotRequired[str | None]
    status: FsEditStatus
    ws: str


class FsEntry(TypedDict):
    """
    `fs.stat`, and entries of `fs.list`.
    """

    gid: int
    kind: FsKind
    mode: int
    mtime: int
    """
    Unix time in ms.
    """
    path: PathName
    """
    Path from the workspace root (`fs.stat`) or name in the directory (`fs.list`).
    """
    size: int
    uid: int


class FsFile(TypedDict):
    """
    `fs.read`.
    """

    content: NotRequired[str | None]
    """
    Base64 content of a regular file (from `offset`).
    """
    entry: FsEntry
    snap: NotRequired[str | None]
    """
    Snapshot read; None for the workspace's directory.
    """
    target: NotRequired[PathName | None]
    """
    Target of a symlink.
    """
    ws: str


class FsList(TypedDict):
    """
    `fs.list`: the entries of a directory.
    """

    at: NotRequired[str | None]
    """
    A changeset or snapshot of the workspace.
    """
    path: NotRequired[PathName | None]
    """
    Directory; default the workspace root.
    """
    ws: str


class FsListing(TypedDict):
    """
    `fs.list`.
    """

    entries: list[FsEntry]
    snap: NotRequired[str | None]
    ws: str


class FsOp1(TypedDict):
    """
    Create or replace a regular file; missing parent directories are created.
    """

    content: str
    """
    Base64.
    """
    mode: NotRequired[int | None]
    """
    Default: the replaced file's mode, else 0644.
    """
    op: Literal['write']
    path: PathName


class FsOp2(TypedDict):
    """
    Create a directory and missing parents.
    """

    mode: NotRequired[int | None]
    op: Literal['mkdir']
    path: PathName


class FsOp3(TypedDict):
    """
    Remove an entry, with everything below it.
    """

    op: Literal['delete']
    path: PathName


FsOp4 = TypedDict(
    'FsOp4',
    {
        'from': PathName,
        'op': Literal['rename'],
        'to': PathName,
    },
)


class FsOp5(TypedDict):
    """
    One change of `fs.edit`. New entries belong to the workspace owner.
    """

    op: Literal['symlink']
    path: PathName
    target: PathName


class FsOp6(TypedDict):
    """
    One change of `fs.edit`. New entries belong to the workspace owner.
    """

    mode: int
    op: Literal['chmod']
    path: PathName


FsOp: TypeAlias = FsOp1 | FsOp2 | FsOp3 | FsOp4 | FsOp5 | FsOp6
"""
One change of `fs.edit`. New entries belong to the workspace owner.
"""


class FsRead(TypedDict):
    """
    `fs.read`: a file's content (from `offset`, at most `length` bytes) or a symlink's target.
    """

    at: NotRequired[str | None]
    """
    A changeset or snapshot of the workspace.
    """
    length: NotRequired[int | None]
    """
    At most this many bytes (default: to the end, up to 64 MiB).
    """
    offset: NotRequired[int | None]
    path: PathName
    ws: str


class FsStat(TypedDict):
    """
    `fs.stat`: one entry.
    """

    at: NotRequired[str | None]
    """
    A changeset or snapshot of the workspace.
    """
    path: PathName
    ws: str


MergeResult = TypedDict(
    'MergeResult',
    {
        'actions': NotRequired[int | None],
        'changeset': NotRequired[str | None],
        'conflicts': NotRequired[list[ConflictInfo]],
        'from': NotRequired[str | None],
        'snap': NotRequired[str | None],
        'status': MergeStatus,
        'unused_resolutions': NotRequired[list[PathName]],
        'ws': str,
    },
)


class ProcChangesetTaken(TypedDict):
    """
    `proc.changeset`.
    """

    changeset: NotRequired[str | None]
    """
    None when nothing changed (see `since`); no snapshot is kept then.
    """
    pause_ms: float
    paused: bool
    """
    Whether the process was frozen (it was running).
    """
    snaps: list[SnapRef]
    unchanged: bool


class ProcMerge(TypedDict):
    discard_ws: NotRequired[list[str]]
    """
    Workspaces whose unmerged changes are dropped (the branch is reset to main).
    """
    no_resolver: NotRequired[bool]
    """
    Report conflicts instead of running the resolver.
    """
    proc_id: str
    resolutions: NotRequired[list[ResolutionSpec]]


class ProcMerged(TypedDict):
    """
    `proc.merge`.
    """

    changeset: NotRequired[str | None]
    """
    Snapshot of the process's branches taken before merging.
    """
    conflict: bool
    id: str
    resolver: NotRequired[str | None]
    """
    What the resolver did, when it ran.
    """
    results: list[MergeResult]
    """
    Merges into main, one per workspace with changes.
    """
    state: str
    """
    The process's state afterwards: running, conflicted, merged or discarded.
    """
    updated: list[MergeResult]
    """
    Running process: main's other changes merged into its branches.
    """


class WsPruneRequest(TypedDict):
    """
    Thin out a workspace's automatic snapshots now (also done periodically).
    """

    args: WsPrune
    op: Literal['ws.prune']


class ResetRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    args: Reset
    op: Literal['reset']


class RevertRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    args: Revert
    op: Literal['revert']


class SessionCreateRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    args: SessionCreate
    op: Literal['session.create']


class ProcMergeRequest(TypedDict):
    """
    The process's task is done: snapshot its branches, merge them into main (the resolver
    handles conflicts) and, while it runs, bring main's other changes into its branches.
    """

    args: ProcMerge
    op: Literal['proc.merge']


class FsReadRequest(TypedDict):
    """
    Read a file (or a symlink's target) of a workspace without mounting it.
    """

    args: FsRead
    op: Literal['fs.read']


class FsListRequest(TypedDict):
    """
    List a directory of a workspace.
    """

    args: FsList
    op: Literal['fs.list']


class FsStatRequest(TypedDict):
    """
    `{"op": "<name>", "args": {...}}`.
    """

    args: FsStat
    op: Literal['fs.stat']


class Applied(TypedDict):
    """
    `reset` and `revert`.
    """

    conflict: bool
    results: list[MergeResult]


class FsEdit(TypedDict):
    author: NotRequired[str | None]
    base: NotRequired[str | None]
    """
    The changeset or snapshot the changes were made against: refuse them if a path
    they touch changed since.
    """
    message: NotRequired[str | None]
    ops: list[FsOp]
    tool_call: NotRequired[str | None]
    ws: str


class LiveSpan(TypedDict):
    """
    A stretch of a workspace's history labelled by a session.
    """

    discard: NotRequired[MergeResult | None]
    """
    `session.discard`: bringing the workspace back to `start_snap`.
    """
    end_snap: NotRequired[str | None]
    """
    None while the session is active.
    """
    start_snap: str
    ws: str


class ProcExited(TypedDict):
    """
    `proc.run` (second response line): the process exited and its work was merged.
    """

    code: NotRequired[int | None]
    """
    Exit code; None when killed by a signal (or unknown after a daemon restart).
    """
    id: str
    merge: ProcMerged
    signal: NotRequired[int | None]


class FsEditRequest(TypedDict):
    """
    Change files of a workspace's main and record the change as one changeset.
    """

    args: FsEdit
    op: Literal['fs.edit']


Request: TypeAlias = PingRequest | WsRegisterRequest | WsListRequest | WsDetachRequest | WsPruneRequest | ChangesetRequest | ResetRequest | RevertRequest | BasePromoteRequest | BaseFlattenRequest | SessionCreateRequest | SessionListRequest | SessionMergeRequest | SessionDiscardRequest | ProcRunRequest | ProcChangesetRequest | ProcMergeRequest | ProcKillRequest | ProcShowRequest | ProcListRequest | LogRequest | DiffRequest | PushRequest | PullRequest | FsReadRequest | FsListRequest | FsStatRequest | FsEditRequest
"""
`{"op": "<name>", "args": {...}}`.
"""


class SessionCreated(TypedDict):
    """
    `session.create`.
    """

    id: str
    live: list[LiveSpan]
    name: NotRequired[str | None]


class SessionEnded(TypedDict):
    """
    `session.merge` and `session.discard`.
    """

    conflict: bool
    """
    `session.discard`: a workspace could not be brought back (see `live[].discard`).
    """
    live: list[LiveSpan]
    session: str
    state: str
    """
    merged or discarded.
    """


class SessionInfo(TypedDict):
    """
    An entry of `session.list`.
    """

    created_at: int
    """
    Unix time in ms.
    """
    id: str
    live: list[LiveSpan]
    name: NotRequired[str | None]
    state: str
    """
    active, merged or discarded.
    """


Results = TypedDict(
    'Results',
    {
        'base.flatten': BaseChanged,
        'base.promote': BaseChanged,
        'changeset': LiveChangeset,
        'diff': list[DiffResult],
        'fs.edit': FsEdited,
        'fs.list': FsListing,
        'fs.read': FsFile,
        'fs.stat': FsEntry,
        'log': list[ChangesetInfo],
        'ping': Pong,
        'proc.changeset': ProcChangesetTaken,
        'proc.kill': ProcKilled,
        'proc.list': list[ProcInfo],
        'proc.merge': ProcMerged,
        'proc.run': ProcStarted,
        'proc.show': ProcInfo,
        'pull': Pulled,
        'push': Pushed,
        'reset': Applied,
        'revert': Applied,
        'session.create': SessionCreated,
        'session.discard': SessionEnded,
        'session.list': list[SessionInfo],
        'session.merge': SessionEnded,
        'ws.detach': WsDetached,
        'ws.list': list[WsInfo],
        'ws.prune': Pruned,
        'ws.register': WsRegistered,
    },
)


class TracefsProtocol(TypedDict):
    """
    tracefs-daemon socket protocol: one JSON request line and one JSON response line per connection
    (`proc.run`: a second line, `ProcExited`).
    """

    proc_exited: ProcExited
    protocol: Literal[2]
    request: Request
    response: Response
    results: Results
