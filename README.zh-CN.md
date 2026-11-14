<div align="center">

🌐 [English](README.md) · **简体中文**

# 🌳 tracefs

### 给 agent 碰过的一切上 git, 做在文件系统里

[![ci](https://github.com/emmettlu/tracefs/actions/workflows/ci.yml/badge.svg)](https://github.com/emmettlu/tracefs/actions/workflows/ci.yml)
![Linux ≥ 6.8](https://img.shields.io/badge/Linux-%E2%89%A5%206.8-FCC624?logo=linux&logoColor=black)
![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust)
![no FUSE](https://img.shields.io/badge/FUSE-none-brightgreen)
![macOS · Windows](https://img.shields.io/badge/tracefs--lite-macOS%20%C2%B7%20Windows%20%C2%B7%20Linux-blue)
![MIT + GPL-2.0](https://img.shields.io/badge/license-MIT%20%2B%20GPL--2.0-lightgrey)

🌿 **每个 agent 一个项目的写时复制分支.**<br>
📸 **每次 tool call 记成一个 changeset.**<br>
🔀 **task 干完再合并, 冲突交给第三个 agent.**<br>
⚡ **不走 FUSE. 每个字节都经过内核文件系统.**

</div>

```
         🧑 你 (vim, cargo, git)            🤖 agent A              🤖 agent B
                 │                             │                       │
                 ▼                             ▼                       ▼
        ~/proj  ── main ──────────────┬──── 分支 A ────┐      ┌── 分支 B
                 ▲  这里的每次写入     │   (自己的        │      │   (同一路径,
                 │  都成为历史         │    mount ns)     │      │    各自一份)
                 │                    │                 ▼      ▼
                 └──────── 三方合并, 跟随重命名 ◄────────────────┘
                           冲突 → 🤖 resolver agent → 🧑 你
```

> 💡 **tracefs** (AgentComputerFS) 是用户态这一侧: `tracefsd`、`tracefs` 命令、SDK 和插件, 和 Linux 自带的 tracefs (`/sys/kernel/tracing`) 无关. 也正因为内核里已经有一个 tracefs, `kernel/` 里的内核文件系统叫 **acfs**, 同名会在内核里冲突.

---

## 🤔 为什么

coding agent 往你的盘上写东西, 经常好几个一起写, 有时就写在你自己也在改的工作目录里. 于是你得回答:

- ❓ *它刚才到底改了什么?* 不是"它说它改了什么".
- ❓ *能不能只撤销那一次 tool call*, 留下之后的十次?
- ❓ *两个 agent 能不能同时改同一个文件*, 互不踩脚?
- ❓ *能不能回滚 agent 干的活*, 又不用关掉目录里开着文件的 shell、编辑器和 dev server?

✅ tracefs 对这四个问题都说能, 而且不需要 agent harness 配合.

## ✨ 有什么

| | |
| --- | --- |
| 🌿 **每个 agent 一个分支** | `tracefs run -- <agent>` 让 agent 跑在自己的 mount namespace 里. 它碰到的每个工作区都换成它自己的 btrfs 快照, 挂在**同一个绝对路径**上, 工具输出里的路径不用转换. 建分支是 O(1) 的, 不复制数据. |
| 📸 **changeset, 不是 commit** | 每次 tool call 可以是 agent 所有分支的一个原子快照. 拍快照时冻结这个进程 (改了 1 万个文件时, 原型 41~56 ms, 现在的守护进程 91 ms), 别的进程不停. 不用 `git add`. |
| 👀 **一个改动都不漏** | 目录注册之后, tracefs 把它的 `main` 挂在上面. 你、你的编辑器、构建脚本的改动, 写停下后都记成 `auto` changeset. 撤销经挂载写回, 目录里开着的文件和跑着的进程不受影响. |
| 🔀 **跟随重命名的合并** | 三方合并先按**文件身份** (同一快照谱系内的 inode 号) 认, 再按内容认. 一个 agent 改了目录名, 另一个改了目录里的文件, 那处修改落在新路径上. 文本冲突交给 `git merge-file`. |
| 🤖 **第三个 agent 解决冲突** | `--resolver '<命令>'` 拿到一个目录: `TASK.md`、`conflicts.json`, 每个冲突一份 `base`/`ours`/`theirs`. 它写 `resolutions.json`, tracefs 再合一次. 它解决不了的留给你. |
| ⏪ **任意粒度的撤销** | `reset` 到任意一点, `revert` 一个 changeset、一段 (一轮 agent) 或整个 session. 每次撤销本身也是新的 changeset, 历史只增不减. |
| 📜 **合并规则** | `.tracefsignore` 让构建产物不进历史、不进合并. `.tracefsmerge` 按模式定策略: `ours`、`theirs`、`newer`、`union`, 或 `unit=<名>` (一组文件一起决定, 锁文件总是和它的清单取同一边). |
| ☁️ **增量 push/pull** | 快照以 `btrfs send -p` 流加 sha256 清单推到任意目录: 本地盘、NFS 或 S3 挂载. |
| 🧹 **历史不膨胀** | 自动快照 24 小时内全留, 之后 7 天内每小时留一个, 再往后每天一个. `log` 和 `diff` 的摘要永远保留. |
| 💥 **崩溃安全** | 改写分支前先写 journal. tracefsd 重启时 agent 照常跑, 之后 tracefsd 把它们认回来. |
| 🐧 **一个内核文件系统** | `kernel/` 是另一个 in-tree 的 Linux 文件系统: 原生分支、跳过共享子树的 diff ioctl、共享块的复制. 见[下面](#-附赠-一个内核文件系统). |

## ⚡ 30 秒上手

```sh
# 1️⃣ 一次性: 把目录交给 tracefs (必须在你的 home 下面)
tracefs ws register proj --from ~/proj

# 2️⃣ 起两个 agent: 都看到 ~/proj, 各有各的分支
tracefs run -- pi        &
tracefs run --resolver 'my-agent "follow TASK.md"' -- claude

# 3️⃣ 同时你自己也改 ~/proj, 一样被记下
tracefs log --ws proj -n 20
tracefs diff 01JD3K…                 # 一个 changeset 改了什么
tracefs diff 01JD3K… --patch         # …带 unified diff

# 4️⃣ 那次 tool call 搞错了
tracefs revert 01JD3K…

# 5️⃣ 整个 session 都搞错了
tracefs revert --session refactor-auth
```

🏁 agent 说一个 task 干完了 (`tracefs proc merge $TRACEFS_PROC`) 或退出时, 它的分支合进 `main`; 这期间别人合进去的工作再同步回它的分支, 它接着干.
🤝 两个 agent 改同一个文件不算错: 先干完的直接合进去, 只有后合的那次可能冲突.

## 🔧 怎么做的

```
/var/lib/tracefs/
├── 🗃️ meta.db           SQLite: 工作区、分支、快照、changeset、journal
├── 🧱 bases/<id>/       只读子卷: l/1 … l/n
├── 🌿 branches/<id>/    一个可写子卷: l/1…l/n + upper/ + work/
└── 📸 snaps/<id>/       分支在某个 changeset 时的只读快照
```

- 🥞 **数据路径: btrfs 上的 overlayfs.** 每个分支是**一个**子卷, 里面放各 lower 层、`upper/` 和 `work/`, 合并视图里 inode 号不重复, `find` 和 `cp -a` 都正常. copy-up 走 reflink.
- 🌿 **分支和快照: btrfs 子卷快照.** O(1), 块级写时复制, 顺带就是增量 send 流.
- 👂 **改动检测: 监听 `upper/` 的 inotify.** 经 overlay 的写入都落在那里, `upper/` 只含改过的目录, watch 数量小.
- 🔍 **快速 diff: `btrfs send --no-data -p A B`.** 内核只走两棵树不同的部分, 代价随改动量而不是树的大小. 完整遍历 upper 的 v1 留作对照: 测试里每次 diff 两个都跑, 不一致即失败.
- ✍️ **合并经挂载写入**, 路径用 `openat2(RESOLVE_BENEATH | NO_SYMLINKS | NO_XDEV)`. 整份取一边内容的文件用 `copy_file_range` 写, btrfs 把它变成 clone: 不占新空间、不写数据. reset 一个 1 GiB 的文件 1.3 s, 结果和快照共用 extent.
- 🧊 **agent: `systemd-run --scope`.** agent 以你的身份、用你的环境, 跑在私有的 mount namespace 里; cgroup 用来在拍快照时冻结它, 以及收拾它留下的进程.
- 🩹 **崩溃恢复.** 启动时 tracefsd 重新挂载工作区, 把做到一半的操作做完或记下来, 按 pid 和启动时间认回还在跑的 agent, 把它停机期间结束的 agent 合并掉.

📖 所有细节 (不变量、每个决定的理由) 都在 [AGENTS.md](AGENTS.md).

## 📊 数字

实测, 不是估算. 负载和机器见 [AGENTS.md §8](AGENTS.md#8-性能指标).

| | |
| --- | --- |
| ⏸️ changeset 暂停, 改了 1 万个文件 | **41~56 ms** (原型, 3 个工作区), **91 ms** (现在的守护进程); 其他进程最大停顿 **0.7 ms** |
| 🚀 `tracefs run` 到命令开始跑, 3 个工作区 | **170 ms** |
| 🔍 diff 一个 10 文件的 changeset | **17 ms** |
| 📁 合并一个改了名的 2500 文件目录 | **154 ms** |
| 🤖×8 个 agent 改完同一工作区后同时退出 | 全部合完、无冲突, **771 ms** |
| 🦀 `cargo build`, fio 4k 随机写 | **与原生 btrfs 持平** |
| 🔎 `rg` 全仓 | 原生 **+4%** |

⚠️ 说实话的那个数: 10 万文件的 `git status` 比原生慢 14~20%, 是 overlay 元数据的开销. 开 `core.untrackedCache` / `core.fsmonitor` 有帮助.

## 🥊 和同行比

### 🏁 正面对比: 同一台机器, 同一份数据

三列在同一台机器上、对同一组 base 树跑, 各自按实际用法挂载: x86_64 NVMe, btrfs (`compress=zstd:1`), Linux 内核源码 (约 10 万个文件) 和 ripgrep 仓库, 用 hyperfine 计时 (脚本 [`tests/kernel/t07_perf.sh`](tests/kernel/t07_perf.sh)).
**fuse-overlayfs** 是同样的分层设计换成走 FUSE, 所以后两列的差距就是每个操作都回一趟用户态的代价.

| 负载 | 🥇 原生 btrfs | ⚡ tracefs (内核 overlayfs) | 🐢 fuse-overlayfs |
| --- | --- | --- | --- |
| 🔎 `rg` 全仓 | 基准 | **+4%** | 比 tracefs **慢 6.3 倍** |
| 📂 `git status`, 冷缓存 | 基准 | **+14%** | 比 tracefs **慢 87 倍** |
| 📂 `git status`, 热缓存 | 基准 | +17~20% | 没有记录 |
| ✍️ buffered 写 | 基准 | **≈ 原生** | 比 tracefs **慢 1.7 倍** |
| 🦀 `cargo build` (中型 crate) | 基准 | **≈ 原生** | 没有记录 |
| 🎲 fio 4k 随机写 | 基准 | **≈ 原生** | 没有记录 |

这些是原型在一台已经停用的机器上跑的, 原始输出不在本仓库里. 别的工具 (git worktree、容器、其他 agent 文件系统) 这里**没有**测过, 所以不给数字. 🙅

### 🧭 各方案的做法

| | 🌳 tracefs | 🌲 `git worktree` | 📋 复制目录 | 🐢 FUSE overlay | 🐳 容器 overlay |
| --- | --- | --- | --- | --- | --- |
| 建一个分支的代价 | **O(1)** 快照 | 检出每个跟踪的文件 | 复制每个字节 | O(1) | O(1) |
| 未跟踪和忽略的文件也带上 | ✅ | ❌ 只有提交过的 | ✅ | ✅ | ✅ |
| 不用 commit 就记下改动 | ✅ 每次 tool call + 自动 | ❌ | ❌ | ❌ | ❌ |
| 你自己对目录的改动也记下 | ✅ | ❌ | ❌ | ❌ | ❌ |
| 每个 agent 看到**同一路径** | ✅ mount namespace | ❌ 另一个目录 | ❌ 另一个目录 | ❌ 另一个挂载点 | ✅ 在容器里 |
| 合并回来 | ✅ 三方, 跟随重命名, resolver agent | 先 commit 才行 | 🖐️ 手工 | 🖐️ 手工 | 🖐️ 手工 |
| 撤销一次 tool call | ✅ | ❌ | ❌ | ❌ | ❌ |
| 数据路径 | ⚡ 内核 | ⚡ 内核 | ⚡ 内核 | 🐢 每个操作都回用户态 | ⚡ 内核 |
| agent 用你的 shell、工具和环境 | ✅ | ✅ | ✅ | ✅ | ❌ 要镜像 |

## 🔌 插件: macOS 和 Windows 上今天就能用

`sdk/portable` 给 agent 插件一套接口, 两种后端:

| 后端 | 什么时候 | 怎么存 |
| --- | --- | --- |
| 🐧 **tracefsd** | Linux, 在 `tracefs run` 里 | 守护进程, 如上 |
| 🪶 **tracefs-lite** | 其余情况; 不要 root, 不要虚拟机 | 🍎 macOS: APFS 稀疏镜像 + `clonefile`<br>🐧 Linux: btrfs 或 btrfs 镜像 + reflink<br>🪟 Windows: NTFS VHDX |

```sh
npm install -g ./sdk/portable              # 把 tracefs-lite 装进 PATH
tracefs-lite init
tracefs-lite register proj ~/proj            # 打印要在里面干活的路径
tracefs-lite branch proj agent1
tracefs-lite merge proj agent1 --prefer theirs
```

- 🥧 **[pi](https://github.com/badlogic/pi-mono)**: `pi -e sdk/pi/index.ts`. 每次写了文件的 assistant 回复记成一个 changeset, 写进会话; `/tree` 和 `/fork` 把文件带回那个点; `/tracefs status|log|diff|undo|merge`.
- 🐋 **DeepSeek Harness**: `dsh plugin --profile <名> add ./sdk/dsh`. 每一步一个 changeset, 每轮结束时合并; 还给模型一个 `tracefs` 工具, 让它能看和撤销自己的活.
- 🧩 **Codex**: `codex plugin marketplace add emmettlu/tracefs`, 再装 `tracefs`. hooks 把每次 `Bash` / `apply_patch` / `Edit` / `Write` 调用记成一个 changeset, `Stop` 时合并; skill `tracefs` 让模型能 status / log / diff / undo / merge. 插件的 hooks 要在 Codex 里审核信任后才跑.
- 🪝 **别的 harness**: 在 hook 里调 `tracefs changeset --author <harness> --tool-call <id>` (例如 Claude Code 的 `PostToolUse`).

## 🧰 SDK

经 `/run/tracefs/tracefs-daemon.sock`, 每个请求一行 JSON. 类型由 [`schema/tracefs.schema.json`](schema/tracefs.schema.json) 生成, 而 schema 由 Rust 的协议类型生成, 两边不会对不上. 🔒

```ts
// 🟦 TypeScript
import { Client } from "@dopamine-org/tracefs";
const log = await new Client().call("log", { ws: "proj", n: 10 });   // 完整类型
```

```python
# 🐍 Python, 零依赖
from tracefs_sdk import Client
diff = Client().call("diff", {"a": "01JD3K…", "patch": True})
```

## 🐧 附赠: 一个内核文件系统

`kernel/` (GPL-2.0) 是同一思路做成的独立 Linux 文件系统 **acfs**: `mount -t acfs`, 分支是一等公民.

```sh
mkfs.acfs /dev/sdX
mount -t acfs                  /dev/sdX /work   # main
mount -t acfs -o branch=agent1 /dev/sdX /a1     # 同一设备的另一个分支
```

- 🌲 带惰性引用计数的写时复制 B+ 树; 建分支就是给一个根加一个引用.
- 🎛️ `ACFS_IOC_BRANCH_CREATE/DELETE/LIST/OPEN`, 以及 `ACFS_IOC_DIFF`: 两棵树一起走, 跳过所有共享的子树.
- 🧬 `copy_file_range`、`FICLONE`、`FIDEDUPERANGE` 共享块而不复制, 跨分支也行.
- 🪪 同一个文件在共享它的每个分支里 inode 号相同; 不同分支各自新建的文件不会撞号.
- 🛡️ 两个轮流写的超级块槽, 全部元数据带 crc32c, 提交用 preflush + FUA, 不靠日志也崩溃一致.
- 👤 用户能给自己拥有的文件系统拍快照, 不需要 `CAP_SYS_ADMIN`.
- 🧪 自带 `mkfs.acfs`、`fsck.acfs` 和在 QEMU 里跑的 selftests (basic、concurrent、crash、stress).

📖 盘上格式和 ABI: [`kernel/Documentation/userspace-api/ac/acfs.rst`](kernel/Documentation/userspace-api/ac/acfs.rst).

## 📦 安装

**要求:** 🐧 Linux ≥ 6.8, 带 btrfs、overlayfs 和 loop; systemd; `btrfs-progs`、`git`、`diffutils`.
💾 状态目录不必在 btrfs 上: 不在时 tracefsd 自己建一个稀疏的 btrfs 镜像, 用开了 direct I/O 的 loop 挂上.

```sh
cargo build --release
sudo bash deploy/install.sh        # tracefsd.service + tracefs.slice 限额
```

- 🏹 Arch 包: `packaging/arch/PKGBUILD`.
- 🍎🪟 macOS/Windows: 用 tracefs-lite (见上). `tracefs-core` 在那边也能编和测.

🧪 **测试.** CI 跑:
- Linux、macOS、Windows 上的插件和 Rust;
- 15 组要 root 的集成测试, 用真的 btrfs 和 systemd, 每组一个用完即扔的 loop 镜像, 有内存压力保护;
- 固定种子的随机操作 fuzzer, 拿本地重放对照 diff/revert/reset/merge;
- 内核模块在 QEMU/KVM 里的 selftests.

见 [AGENTS.md › 测试](AGENTS.md#测试).

## 🚫 它不是什么

- 🔓 **不是沙箱.** agent 以你的身份跑, 你能做的它都能做, 只是它看到的工作区换成了它自己的分支. 要防恶意 agent, 用虚拟机.
- 📡 **不实时共享.** agent 之间看不到对方没合并的写入, 只在合并时共享, 这是设计如此.
- 🧾 **不逐次写入审计.** 历史的粒度是 changeset.

## 📄 许可证

用户态: [MIT](LICENSE). `kernel/`: [GPL-2.0](kernel/COPYING).

---

<div align="center">🌐 <a href="README.md">English</a> · <b>简体中文</b> · <a href="#-tracefs">⬆️ 回到顶部</a></div>
