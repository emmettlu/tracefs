export interface Change {
  ws: string;
  kind: "add" | "modify" | "delete" | "rename";
  path: string;
}

export interface Applied {
  conflict: boolean;
  conflicts: string[];
  changeset?: string | null;
}

export interface LogEntry {
  id: string;
  author: string;
  message: string;
  created_at: number;
  tool_call: string | null;
}

export interface CheckpointOptions {
  toolCall?: string | null;
  since?: string | null;
  author?: string;
}

export interface Backend {
  readonly kind: "acfsd" | "portable";
  describe(): string;
  checkpoint(message: string, options?: CheckpointOptions): Promise<string | undefined>;
  diff(a: string, b?: string | null): Promise<Change[]>;
  undo(changeset: string): Promise<Applied>;
  revert(from: string, to: string): Promise<Applied>;
  reset(target: string): Promise<Applied>;
  log(n?: number): Promise<LogEntry[]>;
  settle(options?: { author?: string }): Promise<{ state: string; conflicts: string[] }>;
  status(): Promise<string>;
}

export declare class DaemonBackend implements Backend {
  readonly kind: "acfsd";
  proc: string;
  socket: string;
  constructor(proc: string, socket?: string);
  static available(env?: NodeJS.ProcessEnv): string | null;
  call(op: string, args?: unknown): Promise<any>;
  describe(): string;
  checkpoint(message: string, options?: CheckpointOptions): Promise<string | undefined>;
  diff(a: string, b?: string | null): Promise<Change[]>;
  undo(changeset: string): Promise<Applied>;
  revert(from: string, to: string): Promise<Applied>;
  reset(target: string): Promise<Applied>;
  log(n?: number): Promise<LogEntry[]>;
  settle(): Promise<{ state: string; conflicts: string[] }>;
  status(): Promise<string>;
}

export declare class PortableBackend implements Backend {
  readonly kind: "portable";
  store: Store;
  ws: string;
  tree: string;
  readonly path: string;
  constructor(store: Store, ws: string, tree: string);
  describe(): string;
  checkpoint(message: string, options?: CheckpointOptions): Promise<string | undefined>;
  diff(a: string, b?: string | null): Promise<Change[]>;
  undo(changeset: string): Promise<Applied>;
  revert(from: string, to: string): Promise<Applied>;
  reset(target: string): Promise<Applied>;
  log(n?: number): Promise<LogEntry[]>;
  settle(options?: { author?: string }): Promise<{ state: string; conflicts: string[] }>;
  status(): Promise<string>;
}

export declare function open(options?: { cwd?: string; env?: NodeJS.ProcessEnv }): DaemonBackend | PortableBackend | null;

export interface StoreChangeset {
  id: string;
  ws: string;
  tree: string;
  author: string;
  message: string;
  tool_call: string | null;
  since: string | null;
  created_at: number;
  taken: number;
  pruned?: boolean;
}

export interface Conflict {
  path: string;
  reason: string;
}

export declare class Store {
  root: string;
  constructor(root: string);
  static init(root: string): Store;
  static find(dir: string): { store: Store; ws: string; tree: string } | null;
  treePath(ws: string, tree?: string): string;
  snapPath(id: string): string;
  head(ws: string, tree: string): string | undefined;
  workspaces(): { name: string; trees: string[]; path: string }[];
  register(ws: string, from?: string): string;
  unregister(ws: string): void;
  checkpoint(ws: string, tree: string, options?: { author?: string; message?: string; toolCall?: string | null; since?: string | null; force?: boolean }): string | null;
  diff(a: string, b?: string | null): { ws: string; from: string; to: string | null; changes: { kind: string; path: string }[] };
  revert(options: { changeset?: string | null; from?: string | null; to?: string | null; author?: string }): { conflict: boolean; conflicts: Conflict[]; changeset: string | null };
  reset(target: string, options?: { author?: string; tree?: string | null }): { conflict: boolean; changeset: string | null };
  log(options?: { ws?: string | null; tree?: string | null; n?: number }): StoreChangeset[];
  branch(ws: string, name: string): string;
  dropBranch(ws: string, name: string): void;
  merge(ws: string, name: string, options?: { author?: string; prefer?: "ours" | "theirs" | null }): { status: string; conflicts: Conflict[]; changeset: string | null; actions?: number };
  prune(ws: string, keep?: number): number;
}

export declare class AcfsLiteError extends Error {
  code: string;
}
export declare class AcfsError extends Error {
  code: string;
}
export declare const META: string;
export declare const DEFAULT_SOCKET: string;
export declare const DEFAULT_SIZE: string;
export declare function call(socket: string, op: string, args?: unknown): Promise<any>;
export declare function pathName(p: string | { base64: string }): string;

export interface StoreState {
  kind: "apfs-image" | "btrfs-image" | "btrfs" | "ntfs-image" | "dir";
  root: string;
  image?: string;
  mount?: string;
  device?: string;
  warning?: string;
}
export declare function ensure(options?: { env?: NodeJS.ProcessEnv; size?: string; plain?: boolean }): StoreState;
export declare function detach(options?: { env?: NodeJS.ProcessEnv }): boolean;
export declare function home(env?: NodeJS.ProcessEnv): string;
export declare function readState(home?: string): StoreState | null;
export declare function fsType(dir: string): string | null;
