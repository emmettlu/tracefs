<div align="center">

🌐 **English** · [简体中文](README.zh-CN.md)

# 🌳 tracefs

### git for everything your agents touch, built into the file system

[![ci](https://github.com/emmettlu/tracefs/actions/workflows/ci.yml/badge.svg)](https://github.com/emmettlu/tracefs/actions/workflows/ci.yml)
![Linux ≥ 6.8](https://img.shields.io/badge/Linux-%E2%89%A5%206.8-FCC624?logo=linux&logoColor=black)
![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust)
![no FUSE](https://img.shields.io/badge/FUSE-none-brightgreen)
![macOS · Windows](https://img.shields.io/badge/tracefs--lite-macOS%20%C2%B7%20Windows%20%C2%B7%20Linux-blue)
![MIT + GPL-2.0](https://img.shields.io/badge/license-MIT%20%2B%20GPL--2.0-lightgrey)

🌿 **Give every agent its own copy-on-write branch of your project.**<br>
📸 **Record every tool call as a changeset.**<br>
🔀 **Merge when the task is done, and let a third agent settle the conflicts.**<br>
⚡ **No FUSE. Every byte goes through kernel file systems.**

</div>

```
         🧑 you (vim, cargo, git)          🤖 agent A              🤖 agent B
                 │                             │                       │
                 ▼                             ▼                       ▼
        ~/proj  ── main ──────────────┬──── branch A ──┐      ┌── branch B
                 ▲  every write here  │   (own mount    │      │   (same path,
                 │  becomes history   │    namespace)   │      │    own copy)
                 │                    │                 ▼      ▼
                 └──────── 3-way merge, follows renames ◄──────┘
                           conflicts → 🤖 resolver agent → 🧑 you
```

> 💡 **tracefs** (AgentComputerFS) is the user-space side: `tracefsd`, the `tracefs` CLI, the SDKs and the plugins. It has nothing to do with Linux's own tracefs at `/sys/kernel/tracing`. Because of that one, the kernel file system in `kernel/` is called **acfs**: two file systems with the same name would collide in the kernel.

---

## 🤔 Why

Coding agents write to your disk. Several of them at once, sometimes in your working tree while you're also in it. Then you get to answer:

- ❓ *What did it just change?* Not "what did it say it changed".
- ❓ *Can I undo that one tool call* and keep the ten after it?
- ❓ *Can two agents edit the same file at the same time* without stepping on each other?
- ❓ *Can I roll back what an agent did* without killing the shell, the editor and the dev server that have files open in that directory?

✅ tracefs answers yes to all four, without asking the agent harness to cooperate.

## ✨ What you get

| | |
| --- | --- |
| 🌿 **A branch per agent** | `tracefs run -- <agent>` starts the agent in its own mount namespace. Every workspace it touches becomes its own btrfs snapshot, mounted at the **same absolute path**, so tool output and paths need no translation. Creating one is O(1): no data is copied. |
| 📸 **Changesets, not commits** | Each tool call can be an atomic snapshot of all the agent's branches together. The process is frozen while it's taken (41~56 ms with 10k changed files in the prototype, 91 ms in the current daemon) and nothing else pauses. Nothing to `git add`. |
| 👀 **Nothing gets past it** | Once a directory is registered, tracefs mounts its `main` over it. Edits from you, your editor or a build script are recorded as `auto` changesets once the writes stop. Undo writes back through the mount, so open files and running processes in the directory aren't disturbed. |
| 🔀 **A merge that follows renames** | Three-way merge by **file identity** (inode numbers within one snapshot lineage), then by content. If one agent renames a directory while another edits a file inside it, the edit ends up at the new path. Text conflicts go to `git merge-file`. |
| 🤖 **A third agent resolves conflicts** | `--resolver '<cmd>'` gets a directory with `TASK.md`, `conflicts.json` and `base`/`ours`/`theirs` for each conflict. It writes `resolutions.json`, and tracefs merges again. Anything it can't resolve is left for you. |
| ⏪ **Undo at any granularity** | `reset` to any point, `revert` one changeset, a range (one agent turn), or a whole session. Every undo is itself a new changeset, so history only grows. |
| 📜 **Merge rules** | `.tracefsignore` keeps build output out of history and out of merges. `.tracefsmerge` sets a strategy per pattern: `ours`, `theirs`, `newer`, `union`, or `unit=<name>`, which decides a group of files together, so a lockfile always comes from the same side as its manifest. |
| ☁️ **Incremental push/pull** | Snapshots ship as `btrfs send -p` streams plus a sha256 manifest, to any directory: local disk, NFS, or an S3 mount. |
| 🧹 **History that stays small** | Automatic snapshots are kept in full for 24 h, then hourly for 7 days, then daily. Their `log` and `diff` summaries are kept forever. |
| 💥 **Crash-safe** | Branch rewrites are journaled. Agents keep running if tracefsd restarts, and tracefsd finds them again afterwards. |
| 🐧 **A kernel file system** | `kernel/` is a separate in-tree Linux file system with native branches, a diff ioctl that skips shared subtrees, and block-sharing copies. See [below](#-bonus-a-kernel-file-system). |

## ⚡ 30 seconds

```sh
# 1️⃣ once: hand a directory to tracefs (it must be under your home)
tracefs ws register proj --from ~/proj

# 2️⃣ start two agents; each sees ~/proj, each gets its own branch
tracefs run -- pi        &
tracefs run --resolver 'my-agent "follow TASK.md"' -- claude

# 3️⃣ meanwhile, edit ~/proj yourself; it's recorded too
tracefs log --ws proj -n 20
tracefs diff 01JD3K…                 # what one changeset did
tracefs diff 01JD3K… --patch         # …with unified diffs

# 4️⃣ that tool call was a mistake
tracefs revert 01JD3K…

# 5️⃣ the whole session was a mistake
tracefs revert --session refactor-auth
```

🏁 When an agent says a task is done (`tracefs proc merge $TRACEFS_PROC`), or when it exits, its branches are merged into `main`. Work that other agents merged in the meantime is merged back into its branch, and it keeps going from there.
🤝 Two agents editing the same file is not an error: whoever finishes first merges cleanly, and only the second merge can conflict.

## 🔧 How it works

```
/var/lib/tracefs/
├── 🗃️ meta.db           SQLite: workspaces, branches, snaps, changesets, journal
├── 🧱 bases/<id>/       read-only subvolume: l/1 … l/n
├── 🌿 branches/<id>/    one writable subvolume: l/1…l/n + upper/ + work/
└── 📸 snaps/<id>/       read-only snapshot of a branch at a changeset
```

- 🥞 **Data path: overlayfs on btrfs.** Each branch is a *single* subvolume holding its lower layers, `upper/` and `work/`. Inode numbers stay unique in the merged view, so `find` and `cp -a` behave. Copy-up is a reflink.
- 🌿 **Branches and snapshots: btrfs subvolume snapshots.** They take O(1) time, are copy-on-write at block level, and work as incremental send streams for free.
- 👂 **Change detection: inotify on `upper/`.** Every write through the overlay lands there, and `upper/` only contains directories that changed, so the number of watches stays small.
- 🔍 **Fast diff: `btrfs send --no-data -p A B`.** The kernel walks only the parts of the two trees that differ, so the cost depends on the size of the change. The full upper-layer walk (v1) is kept as a check: the test suite runs both on every diff and fails if they disagree.
- ✍️ **Merges write through the mount**, using `openat2(RESOLVE_BENEATH | NO_SYMLINKS | NO_XDEV)`. A file taken whole from one side is written with `copy_file_range`, which btrfs turns into a clone: no new space and no data written. Resetting a 1 GiB file took 1.3 s, and the result shares the snapshot's extents.
- 🧊 **Agents: `systemd-run --scope`.** The agent runs as you, with your environment, in a private mount namespace. The cgroup is used to freeze it for snapshots and to kill whatever it leaves behind.
- 🩹 **Crash recovery.** At startup tracefsd re-mounts workspaces, finishes or records half-done operations, finds running agents again by pid and start time, and merges whatever finished while it was down.

📖 Everything is specified, down to the invariants and the reasoning behind each decision, in [AGENTS.md](AGENTS.md).

## 📊 Numbers

Measured, not projected. Workload and machine are in [AGENTS.md §8](AGENTS.md#8-性能指标).

| | |
| --- | --- |
| ⏸️ changeset pause, 10k changed files | **41~56 ms** (prototype, 3 workspaces), **91 ms** (current daemon); other processes stall at most **0.7 ms** |
| 🚀 `tracefs run` to the command starting, 3 workspaces | **170 ms** |
| 🔍 diff of a 10-file changeset | **17 ms** |
| 📁 merge a renamed 2,500-file directory | **154 ms** |
| 🤖×8 agents exit at once after editing one workspace | all merged, no conflicts, in **771 ms** |
| 🦀 `cargo build`, fio 4k random write | **same as native btrfs** |
| 🔎 `rg` over the whole repo | native **+4%** |

⚠️ One honest number: `git status` on 100k files is 14~20% slower than native, because of overlay metadata. `core.untrackedCache` / `core.fsmonitor` help.

## 🥊 Against the alternatives

### 🏁 Head to head: same machine, same data

The three columns run on the same machine against the same base trees, and each is mounted the way it would be used: x86_64 NVMe, btrfs (`compress=zstd:1`), a Linux kernel checkout (~100k files) and the ripgrep repo, timed with hyperfine (script: [`tests/kernel/t07_perf.sh`](tests/kernel/t07_perf.sh)).
**fuse-overlayfs** is the same layered design served through FUSE, so the gap between the last two columns is the cost of a trip to userspace on every operation.

| workload | 🥇 native btrfs | ⚡ tracefs (kernel overlayfs) | 🐢 fuse-overlayfs |
| --- | --- | --- | --- |
| 🔎 `rg` over the whole repo | baseline | **+4%** | **6.3× slower** than tracefs |
| 📂 `git status`, cold cache | baseline | **+14%** | **87× slower** than tracefs |
| 📂 `git status`, hot cache | baseline | +17~20% | not recorded |
| ✍️ buffered writes | baseline | **≈ native** | **1.7× slower** than tracefs |
| 🦀 `cargo build` (mid-size crate) | baseline | **≈ native** | not recorded |
| 🎲 fio 4k random write | baseline | **≈ native** | not recorded |

These are prototype runs on a machine that has since been retired. The raw output isn't in this repo. Other tools (git worktree, containers, other agent file systems) were **not** benchmarked here, so they get no numbers. 🙅

### 🧭 How the approaches differ

| | 🌳 tracefs | 🌲 `git worktree` | 📋 copy the dir | 🐢 FUSE overlay | 🐳 container overlay |
| --- | --- | --- | --- | --- | --- |
| making a branch costs | **O(1)** snapshot | a checkout of every tracked file | a copy of every byte | O(1) | O(1) |
| untracked & ignored files come along | ✅ | ❌ only what's committed | ✅ | ✅ | ✅ |
| edits recorded with no commit step | ✅ per tool call + automatic | ❌ | ❌ | ❌ | ❌ |
| your own edits to the directory recorded | ✅ | ❌ | ❌ | ❌ | ❌ |
| every agent sees the **same path** | ✅ mount namespace | ❌ another directory | ❌ another directory | ❌ another mount point | ✅ inside the container |
| merging back | ✅ 3-way, follows renames, resolver agent | after you commit | 🖐️ by hand | 🖐️ by hand | 🖐️ by hand |
| undo one tool call | ✅ | ❌ | ❌ | ❌ | ❌ |
| data path | ⚡ kernel | ⚡ kernel | ⚡ kernel | 🐢 userspace on every op | ⚡ kernel |
| agent uses your shell, tools and env | ✅ | ✅ | ✅ | ✅ | ❌ needs an image |

## 🔌 Plugins: works today on macOS and Windows too

`sdk/portable` gives agent plugins one interface with two backends:

| backend | when | storage |
| --- | --- | --- |
| 🐧 **tracefsd** | Linux, inside `tracefs run` | the daemon, as described above |
| 🪶 **tracefs-lite** | everywhere else; no root, no VM | 🍎 macOS: APFS sparse image + `clonefile`<br>🐧 Linux: btrfs or a btrfs image + reflink<br>🪟 Windows: an NTFS VHDX |

```sh
npm install -g ./sdk/portable              # puts tracefs-lite on PATH
tracefs-lite init
tracefs-lite register proj ~/proj            # prints the path to work in
tracefs-lite branch proj agent1
tracefs-lite merge proj agent1 --prefer theirs
```

- 🥧 **[pi](https://github.com/badlogic/pi-mono)**: `pi -e sdk/pi/index.ts`. Every assistant reply that writes files becomes a changeset, recorded in the session. `/tree` and `/fork` bring the files back to that point. `/tracefs status|log|diff|undo|merge`.
- 🐋 **DeepSeek Harness**: `dsh plugin --profile <name> add ./sdk/dsh`. One changeset per step and a merge at the end of each turn. It also gives the model a `tracefs` tool so it can look at and undo its own work.
- 🧩 **Codex**: `codex plugin marketplace add emmettlu/tracefs`, then install `tracefs`. Hooks record a changeset for every `Bash` / `apply_patch` / `Edit` / `Write` call and merge on `Stop`. A `tracefs` skill gives the model status / log / diff / undo / merge. Codex only runs a plugin's hooks after you trust them.
- 🪝 **Anything else**: call `tracefs changeset --author <harness> --tool-call <id>` from a hook (for example Claude Code's `PostToolUse`).

## 🧰 SDKs

One JSON request per line over `/run/tracefs/tracefs-daemon.sock`. The types are generated from [`schema/tracefs.schema.json`](schema/tracefs.schema.json), which is generated from the Rust protocol types, so neither side can drift. 🔒

```ts
// 🟦 TypeScript
import { Client } from "@dopamine-org/tracefs";
const log = await new Client().call("log", { ws: "proj", n: 10 });   // fully typed
```

```python
# 🐍 Python, zero dependencies
from tracefs_sdk import Client
diff = Client().call("diff", {"a": "01JD3K…", "patch": True})
```

## 🐧 Bonus: a kernel file system

`kernel/` (GPL-2.0) is the same idea as its own Linux file system, **acfs**: `mount -t acfs`, with branches as a first-class feature.

```sh
mkfs.acfs /dev/sdX
mount -t acfs                  /dev/sdX /work   # main
mount -t acfs -o branch=agent1 /dev/sdX /a1     # another branch, same device
```

- 🌲 Copy-on-write B+ trees with lazy reference counts. Creating a branch adds one reference to a root.
- 🎛️ `ACFS_IOC_BRANCH_CREATE/DELETE/LIST/OPEN`, and `ACFS_IOC_DIFF`, which walks two trees together and skips every subtree they share.
- 🧬 `copy_file_range`, `FICLONE` and `FIDEDUPERANGE` share blocks, even across branches.
- 🪪 A file has the same inode number in every branch it shares. Files created in different branches never collide.
- 🛡️ Two alternating superblock slots, crc32c on all metadata, commits with preflush + FUA, and crash consistency without a log.
- 👤 Users can snapshot a file system they own without `CAP_SYS_ADMIN`.
- 🧪 Comes with `mkfs.acfs`, `fsck.acfs` and selftests (basic, concurrent, crash, stress) that run in QEMU.

📖 Format and ABI: [`kernel/Documentation/userspace-api/ac/acfs.rst`](kernel/Documentation/userspace-api/ac/acfs.rst).

## 📦 Get it

**Requirements:** 🐧 Linux ≥ 6.8 with btrfs, overlayfs and loop; systemd; `btrfs-progs`, `git`, `diffutils`.
💾 The state directory doesn't have to be on btrfs: if it isn't, tracefsd creates a sparse btrfs image and loop-mounts it with direct I/O.

```sh
cargo build --release
sudo bash deploy/install.sh        # tracefsd.service + tracefs.slice limits
```

- 🏹 Arch packages: `packaging/arch/PKGBUILD`.
- 🍎🪟 macOS/Windows: use tracefs-lite (above). `tracefs-core` also builds and tests there.

🧪 **Tests.** CI runs:
- the plugins and Rust on Linux, macOS and Windows;
- 15 root integration suites against real btrfs and systemd, each on a throwaway loop image, under memory-pressure guards;
- a seeded random-operation fuzzer that checks diff/revert/reset/merge against a local replay;
- the kernel module's selftests under QEMU/KVM.

See [AGENTS.md › 测试](AGENTS.md#测试).

## 🚫 What it isn't

- 🔓 **Not a sandbox.** An agent runs as you and can do anything you can. Only its view of the workspaces is replaced by its own branches. If you need to contain a hostile agent, use a VM.
- 📡 **Not live sharing.** Agents don't see each other's unmerged writes. They share at merge time, by design.
- 🧾 **Not per-write auditing.** The unit of history is the changeset.

## 📄 License

User space: [MIT](LICENSE). `kernel/`: [GPL-2.0](kernel/COPYING).

---

<div align="center">🌐 <b>English</b> · <a href="README.zh-CN.md">简体中文</a> · <a href="#-tracefs">⬆️ top</a></div>
