# tracefs 协作约定

full agentic. `README.md` 只是给外人看的介绍 (数字和说法都取自本文件, 改了这里的设计或性能数字要同步它); 协作约定、设计与现状以本文件为准: 协作约定在前, 设计与现状在后面的「设计」.

tracefs (AgentComputerFS): agent 版本化文件系统. 本仓库 (tracefs) 有两部分: 根目录是用户态 (`tracefsd` 守护进程、`tracefs` CLI、SDK),
`kernel/` 是内核态 (内核文件系统 `acfs`, 见「内核态」). 开源: 用户态 MIT (`LICENSE`), `kernel/` GPL-2.0 (`kernel/COPYING`, 文件里有 SPDX 行).
名字和 Linux 自带的 tracefs (`/sys/kernel/tracing`) 无关. 用户态的程序、命令、包和插件都叫 tracefs; 内核态的文件系统仍叫 acfs (`mount -t acfs`, `fs/acfs/`, `CONFIG_AC_FS`), 因为内核里已经有一个 tracefs 文件系统, 同名会冲突.

`tracefsd` 以 root 运行, 接管注册的目录 (工作区): 目录上挂着它的 `main`, 谁直接改都记成 changeset. agent 用 `tracefs run` 起,
跑在自己的 mount namespace 里, 每个工作区换成它自己的写时复制分支 (btrfs 子卷 + overlayfs); 一个 task 干完时合进 `main`,
冲突交给第三个 agent (resolver). 改动可以列出, 回滚或丢弃. 客户端走 Unix socket (`/run/tracefs/tracefs-daemon.sock`), 每个请求一行 JSON.

## 目录

| 路径 | 内容 |
| --- | --- |
| `crates/tracefs-core` | 协议类型, 合并算法 |
| `crates/tracefs-daemon` | 守护进程, 二进制 `tracefsd` |
| `crates/tracefs-cli` | `tracefs` 命令 |
| `schema/` | socket 协议的 JSON schema, 由 `tracefs-core` 生成 |
| `sdk/typescript`, `sdk/python` | TypeScript (`@dopamine-org/tracefs`) 和 Python (`tracefs-sdk`) 客户端 |
| `sdk/portable` | `tracefs-portable`: 插件共用的后端 (tracefsd 或 tracefs-lite) 和 `tracefs-lite` 命令, 纯 JS, 见「插件」 |
| `sdk/pi` | pi-coding-agent 扩展, 见「插件」 |
| `sdk/dsh` | DeepSeek Harness (dsh) 插件包 `dsh-tracefs`, 见「插件」 |
| `sdk/codex`, `.agents/plugins/marketplace.json` | Codex 插件 `tracefs` (hooks + skill) 和把本仓库当插件源的 marketplace, 见「插件」 |
| `.github/workflows/ci.yml` | GitHub Actions: Linux / macOS / Windows 上的插件和 Rust, Linux 上的 tracefsd 集成测试和内核模块 |
| `tests/` | 集成测试 (root, btrfs, systemd), `stress.py` 随机文件操作, `bench.sh` 基准 |
| `tests/kernel/` | tracefs 依赖的内核/btrfs/overlayfs 行为探针 (`probe.sh`, 「设计」里的「验证 x.y」) 和内核层性能基准 (`t07_perf.sh`); `ovl.c` 是它们用的挂载小工具 |
| `vm/` | macOS 上开发用的 Lima VM (`agentos.yaml`, `setup-guest.sh`); `build-alarm-image.sh` 做 Lima (vz) 能起的 Arch Linux ARM 镜像 `arch-aarch64.qcow2` (agentos, ackernel 的 VM 也用它) |
| `deploy/` | systemd unit 和 `install.sh` |
| `kernel/` | 内核态 acfs, 按内核源码树的布局放 (`fs/acfs/`, `include/uapi/linux/ac/`, `Documentation/userspace-api/ac/acfs.rst`, `tools/testing/selftests/ac/acfs/`) |

## 禁止

- 在脚本或文件里写死密码等凭据.
- force push; rebase 或 amend 已推送的提交.

## 运行要求

Linux >= 6.8 (`lowerdir+`), 带 btrfs, overlayfs 和 loop; systemd (`systemd-run --scope`); `btrfs-progs`, `git`, `coreutils`, `diffutils`.
状态目录不必在 btrfs 上: 不在时 `tracefsd` 首次启动建 `<state>.img` (默认 64G 稀疏, `chattr +C`) 并挂在一个开了 direct I/O、4K 扇区的 loop 设备上 (挂上之后 `losetup -d`, 卸载时自动释放).

## 构建

- `cargo build --release`. 只能在 Linux 上编译 (挂载 API, btrfs ioctl); `tracefs-core` 在 macOS 上也能编和跑单元测试.
- Mac 上写代码; 构建和需要 root 的测试在 Lima VM `agentos` 里跑 (`vm/agentos.yaml`, Arch Linux ARM aarch64, 根分区 btrfs;
  `vm/setup-guest.sh` 准备依赖、`tests/kernel/` 的副本 `~/agentfs-verify` 和测试数据): 仓库以只读方式挂在同一路径, 先
  `rsync --exclude target` 到 guest 目录再构建.
- x86_64: 一台带 btrfs 的 Arch 机器跑集成测试和「设计」第 8 节的基准; 共享的机器跑测试要限资源. 同样 rsync 过去再构建.
- 升级内核后在 guest 里跑 `sudo bash ~/agentfs-verify/probe.sh`, FAIL 说明方案依赖的内核行为变了.
- 系统调用用 rustix; libc 只留给 rustix 没有的 (目前: `SO_PEERPIDFD`, CLI 的 `sigwait`).
- socket 协议的类型以 `tracefs-core` 为准 (请求 `proto.rs`, 结果 `results.rs`, op 与结果的对应 `schema.rs`). 改了先
  `UPDATE_SCHEMA=1 cargo test -p tracefs-core schema` 更新 `schema/tracefs.schema.json` (过期时 `cargo test` 失败),
  再在 guest 里跑 `sdk/gen.sh` 重新生成 `sdk/typescript`, `sdk/python` 的类型, 拷回仓库.
- 提交前: `cargo fmt --check` (`rustfmt.toml`, 行宽 160), `cargo clippy --all-targets -- -D warnings`, `cargo test`. 没有 CI.

## 测试

各平台的测试都在 CI (`.github/workflows/ci.yml`) 里跑, 本地同样的命令 (Node >= 22; dsh 本身要 24.9):

| 部分 | 命令 | 平台 |
| --- | --- | --- |
| tracefs-portable | `cd sdk/portable && node --test "test/*.test.js"`: 仓库操作 (diff、同毫秒的改动、可执行位、软链、二进制、undo / revert / reset 和冲突、分支合并、文本合并、锁、prune)、后端选择、假 tracefsd socket 上的协议、`tracefs-lite` 命令、镜像 (macOS APFS 镜像并量快照占的空间, Linux btrfs / btrfs 镜像要 root 或 udisksctl 加 `TRACEFS_LITE_TEST_DIR` 指到非 btrfs 目录, Windows VHDX 要管理员) | Linux / macOS / Windows |
| dsh-tracefs | `cd sdk/dsh && node --test "test/*.test.js"` (照 dsh 的事件签名写的假 ctx, 真 tracefs-lite 仓库) | 三个都跑 |
| Codex 插件 | `cd sdk/codex && node --test "test/*.test.js"`: 清单 / hooks / marketplace 对得上、`lib/portable` 和 `sdk/portable` 相同、按 Codex 的 hook 输入 JSON 逐个起 `hook.mjs` (changeset、合并、冲突消息、不在仓库里时不出声、坏输入不挡工具)、skill 脚本 | 三个都跑 |
| dsh-tracefs 端到端 | `DSH_SOURCE=<构建好的 deepseek-harness> bash sdk/dsh/test/e2e.sh`: 真 dsh 的 headless profile 加本插件, dsh 自带的 mock 模型 (`llm-mock-server`) 在 tracefs-lite 分支里调 bash, 查 changeset (作者、tool call id) 和回合结束时合进 main. CI 钉住 deepseek-harness 的一个提交 (`DSH_REF`) | Linux / macOS |
| pi 扩展 | `cd sdk/pi && npm install && npx tsc -p . && node test-portable.ts` (真 pi 运行时, faux 模型, tracefs-lite 分支); tracefsd 下是 `node test.ts` (要守护进程, 自己 `tracefs run`) | 三个都跑; tracefsd 那份只在 Linux |
| Rust | `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`; macOS / Windows 只 `cargo test -p tracefs-core` | 三个都跑 |
| tracefsd 集成测试 | 下面的 `tests/run.sh` | Linux (root) |
| 内核模块 | `bash kernel/test.sh [工作目录]`: 下 `KVER` (默认 7.3.4) 的源码, 放进 acfs, `x86_64_defconfig` + `kvm_guest.config` + selftests 的 `config`, 编 bzImage 和 selftests, `vm.sh` 在 QEMU/KVM 里跑 | Linux x86_64 + KVM |

- 单元测试 (`cargo test`, 无 root): 合并决议 (含重命名, 规则), 合并基点, 规则文件, send 流解析, 路径, schema 是否最新, home 查找.
  diff 翻译只有集成测试覆盖.
- 集成测试 `tests/t01..t15` (bash) 需要 root, btrfs, systemd: `tests/setup-host.sh` 检查要用的工具, 然后
  `sudo env SRC=<构建目录> bash tests/run.sh [t01_register ...]` (`CARGO_TARGET_DIR` 在别处时另给 `BIN=<target>/release`),
  结果在 `tests/out/` (`summary` 是每个测试的通过数). 每个测试用独立的 loop btrfs 镜像 (`/var/tmp/afst/<name>/state.img`),
  守护进程用 `--slice afstest` 和 `/run/afst-<name>`, 不碰宿主 `/var/lib`; agent 进程以 uid 1000 跑 `tracefs run`.
- 测试里的守护进程带 `TRACEFS_DIFF_CHECK`: 每个 diff v2 都再跑一遍 v1, 不一致即报错; 每个测试结束时检查日志里没有 diff 退回 v1.
- 随机操作: `tests/stress.py` (固定种子, 同一种子在两棵相同的树上结果相同) 对照本地重放, 并验证 diff, revert, reset,
  flatten 和跟随重命名的合并 (t14).
- 基准: `run.sh bench`, 表格写到 `tests/out/bench.md` (「设计」第 8 节的数字从这里来).
- 测试机安全 (曾把测试机内存耗尽, 只能现场重启):
  - 数据放磁盘 (`/var/tmp/afst`), 不放 tmpfs `/tmp`; 不碰 `/var/lib`.
  - 只用 `run.sh` 跑: 每个测试在限内存的 scope 里, `guard.sh` 在内存, tmpfs, 磁盘或内存压力越线时杀掉并清理.
  - 同一台机器同一时间只跑一个 `run.sh`: unit 名和 `/var/tmp/afst` 全局共用, 换源码目录也不隔离. 残留用 `tests/reap.sh` 清.

## 插件

pi 和 dsh 插件只用用户态. 它们经 `sdk/portable` 的 `open({cwd})` 拿一个后端 (`Backend`: `checkpoint` / `diff` / `undo` / `revert` / `reset` /
`log` / `settle` / `status`), 两种实现接口相同:

| 后端 | 什么时候 | 怎么存 |
| --- | --- | --- |
| `tracefsd` (`DaemonBackend`) | Linux, 进程在 `tracefs run` 里 (有 `TRACEFS_PROC`) 且 socket 在 | 守护进程: btrfs 上直接用子卷; 状态目录不在 btrfs 上时它自己建 `<state>.img` loop 挂成 btrfs |
| `portable` (`PortableBackend`, tracefs-lite) | 其余情况, cwd 在 tracefs-lite 仓库的某棵树里 | 见下表 |

tracefs-lite 不用 VM, 也不要 root (Windows 建 / 挂 VHDX 要管理员). 仓库在 `TRACEFS_LITE_HOME` (默认 `~/.tracefs-lite`, Windows `%LOCALAPPDATA%\tracefs-lite`), `tracefs-lite init` 按平台建:

| 平台 | `kind` | 仓库 | 快照 |
| --- | --- | --- | --- |
| macOS | `apfs-image` | `tracefs.sparseimage` (APFS 稀疏镜像, `hdiutil attach -nobrowse -owners on`) 挂在 `mnt/` | `/bin/cp -c -R -p` (clonefile, 不占空间; Node 的 `COPYFILE_FICLONE` 在 macOS 上不克隆) |
| Linux, `TRACEFS_LITE_HOME` 在 btrfs 上 | `btrfs` | `store/` 目录 | `copyFile` + `FICLONE` (reflink) |
| Linux, 不在 btrfs 上 | `btrfs-image` | `tracefs.img` (`mkfs.btrfs --rootdir`, 卷根归当前用户): root 用 `mount -o loop`, 否则 `udisksctl loop-setup` + `mount` | reflink |
| Windows | `ntfs-image` | `tracefs.vhdx` (diskpart 建 NTFS 分区, 挂在 `mnt\` 文件夹) | 整份复制 (NTFS 没有块克隆) |
| 都不行 / `TRACEFS_LITE_PLAIN=1` | `dir` | `store/` 目录, `warning` 写原因 | 文件系统支持 reflink 时克隆, 否则复制 |

`TRACEFS_LITE_ROOT=<目录>` 直接用那个目录当仓库 (测试用). 仓库根是 `tracefs-lite.json` (工作区、分支、changeset 表), `trees/<ws>/<树>/` 是活的树
(`main` 和各分支), `snaps/<id>/` 是 changeset 的快照; 跨进程用 `.lock/` 目录锁 (持有者死了或超过 2 分钟就收回). 工作区要放进仓库:
`tracefs-lite register <ws> <dir>` 复制进去并打印路径, agent 在那棵树 (或 `tracefs-lite branch <ws> <名>` 的写时复制分支) 里跑.
比较: 类型、大小、可执行位、mtime (毫秒); mtime 不早于旧快照拍摄时刻的文件逐字节比 (同一毫秒里改两次也看得见). 合并是三方的: 只一边改了取那边,
两边都改的文本文件交给 `git merge-file`, 合不了的报冲突 (两棵树都不动), `--prefer ours|theirs` 按一边定; 合并后分支和 main 相同, 基点更新.
`undo` 只撤那个 changeset 碰过的路径, 这些路径之后又变了 (或撤销会删掉之后新加的文件) 就是冲突, 什么都不改. 局限: 没有 mount namespace,
agent 不会被关进自己的分支 (在哪棵树里跑就改哪棵); 操作是同步的, 大树的快照会阻塞插件所在进程.

- **pi** (`sdk/pi/index.ts`, `pi -e sdk/pi/index.ts`): 一次 assistant 回复里写文件的工具调用 (除 `read` / `grep` / `find` / `ls`) 记成一个
  changeset (第一次写之前先记 "before"), 写进会话的 `tracefs-checkpoint` 条目; prompt 结束 (`agent_settled`) 时 `settle`: tracefsd 是 `proc.merge`,
  tracefs-lite 在分支上就合进 main (main 上什么都不做); `/tree`、`/fork` 把文件带回那个点 (`TRACEFS_PI_RESTORE=ask|always|never`); `/tracefs status|log|diff|undo|merge`.
  不在 tracefsd 也不在 tracefs-lite 树里时只有 `/tracefs`, 说明怎么开.
- **dsh** (`sdk/dsh`, `dsh plugin --profile <名> add ./sdk/dsh`): cordis 插件 `tracefs` (inject `tools`, `workingDirectory`). 工作区是会话的
  `workingDirectory`. `tools/pre-execute` 在一步里第一个写文件的工具 (`WRITE_TOOLS`, 配置 `writeTools`) 之前记 "before", `tools/result` 收集,
  `agent/pre-step` (下一步开始) 把上一步的调用记成一个 changeset, `turn/end` 记完并 `settle` (配置 `merge: false` 关掉), 冲突只 warn.
  给模型一个工具 `tracefs` (`action`: status / log / diff / undo / merge). 同一会话的操作排成一队; 并行的工具调用会落进同一个 changeset.
  配置还有 `author` (默认 `dsh`).

- **Codex** (`sdk/codex`, `.codex-plugin/plugin.json`): 本仓库是 Codex 的插件源 (`.agents/plugins/marketplace.json` 指向 `./sdk/codex`).
  hooks (`hooks/hooks.json`, 命令经 `$PLUGIN_ROOT`; Windows 用 `commandWindows` 里的 `node -e`, 不依赖 shell 展开变量): `SessionStart` 告诉模型
  tracefs 在记录; `PreToolUse` (matcher `Bash|apply_patch|Edit|Write`) 在这一次调用前记 "before"; `PostToolUse` 把这次调用记成一个 changeset
  (作者 `codex`, `tool_call` 是 `tool_use_id`, 消息是命令首行或 patch 里的文件); `Stop` 时 `settle` (`TRACEFS_CODEX_MERGE=0` 关), 冲突回
  `systemMessage`. 会话状态 (`last` / `before` / 历史) 在 `$PLUGIN_DATA/sessions/<session_id>.json`, 每个 hook 是单独的进程. skill `tracefs`
  (`skills/tracefs/SKILL.md` + `scripts/tracefs.mjs`): status / log / diff / undo / merge. Codex 装插件是复制整个目录, 所以 `lib/portable` 是
  `sdk/portable` 的副本: 改了 `sdk/portable` 要跑 `sdk/sync-portable.sh` (同时更新 `sdk/dsh/lib/portable`) (测试比对两份, 不一致就失败). 装法: `codex plugin marketplace add emmettlu/tracefs`,
  再装 `tracefs`; 插件的 hooks 要在 Codex 里审核信任后才跑.

### 上架 DSH

DSH 没有官方的中央市场: Discover 读用户加的索引 URL (format 1, 见 deepseek-harness 的 `dsh-plugin-market-index`). 本仓库自己发一个:
`market/index.json`, 地址 `https://raw.githubusercontent.com/emmettlu/tracefs/main/market/index.json`. 版本的安装源是 Git (`github.com/emmettlu/tracefs`,
钉住提交, `path: sdk/dsh`), 不用 npm; 图标内联成 data URI (raw.githubusercontent 给 SVG 的类型是 text/plain, 页面显示不了). 发新版: 改
`sdk/dsh/package.json` 的 `version`, 提交推送, 再 `node market/gen.mjs <那个提交>` (旧版本保留在 `versions`, `latest` 指新版) 并提交.
CI 的 dsh-e2e 用 dsh 的解析器校验索引 (`market/validate.ts`), 按它给的安装串装进一个 profile 并加载. 仓库带 GitHub topic `dsh-plugin`.

## 内核态

`kernel/` 是一个带分支的写时复制内核文件系统 (`mount -t acfs`), 用法、ioctl 和盘上格式见 `kernel/Documentation/userspace-api/ac/acfs.rst`.
它在内核树里编 (in-tree): 把 `kernel/` 下的四个目录拷进内核源码树, 在 `fs/Kconfig` 加 `source "fs/acfs/Kconfig"`, 在 `fs/Makefile` 加
`obj-$(CONFIG_AC_FS) += acfs/`, 打开 `CONFIG_AC_FS` (要 4K 页). 测试: `make -C tools/testing/selftests TARGETS=ac/acfs` 编出静态的测试和
`mkfs.acfs` / `fsck.acfs`, 再用 `tools/testing/selftests/ac/acfs/vm.sh <bzImage> <测试目录>` 在 QEMU 里跑.

## 部署

在构建目录 `sudo bash deploy/install.sh`, 装成 root:root 的 `tracefsd.service`; 鉴权见「设计」第 7 节.
测试用的 `--slice afstest` 和 `/run/afst-*` 与之互不影响.

## 打包与发版

- Arch 包: `packaging/arch/PKGBUILD`, 从本仓库 `HEAD` 打 (`git archive`), 未提交的改动不进包. 装 `/usr/bin/tracefsd`, `/usr/bin/tracefs`,
  systemd unit (不自动 enable), `tracefs.slice` 限额, tmpfiles (`/var/lib/tracefs`, btrfs 上是子卷), pi 扩展的源码
  (`/usr/share/tracefs/sdk/pi/index.ts` 和它 import 的 `/usr/share/tracefs/sdk/typescript/src/`, 用法 `pi -e /usr/share/tracefs/sdk/pi/index.ts`;
  AgentOS 桌面的 Milo 等 pi 进程用 `tracefs run` 起, 靠它把每次工具调用记成 changeset, prompt 结束时合并). `check()` 只跑单元测试.
- 发版 (没有 CI): 打 `v*` tag 并推送, 然后在 x86_64 Arch 机器和 Arch Linux ARM 机器 (如 Lima VM) 上各 checkout 这个 tag
  跑一次 `packaging/arch/release.sh <tag>`: 打本机架构的包和 `.sha256`; `gh` 已登录就建 GitHub release 并上传, 否则打印要在别处跑的命令.
- 版本: tag `v1.2.3` 即包版本 `1.2.3`; tag 之后的提交是 `1.2.3.r<数>.g<hash>`.

## 代码

Rust、TypeScript 和 JavaScript 代码不写注释 (`//`, `/* */`, `///`, `//!`, JSDoc 都不写), 要说明的写进本文件. 例外: 版权和许可证行,
工具要读的指令 (`/// <reference>`, `@ts-expect-error` 这类).
clap 的帮助和 schemars 的描述写成属性 (`#[command(about = ...)]`, `#[arg(help = ...)]`, `#[schemars(description = ...)]`),
不写成文档注释; 改了 schemars 的描述要 `UPDATE_SCHEMA=1 cargo test -p tracefs-core schema` 并重新生成 SDK (`sdk/gen.sh`).

守护进程里几个共用的部件:

- `ops::snap_branches`: 给一组分支各拍一个只读快照, 失败时由 `Undo` 删掉已拍的; 自动快照、`changeset`、`proc changeset`、
  进程的各种「before ...」快照都走它, 再各自决定比不比、记不记 changeset.
- `ops::merge_into`: 5.7 的合并加上 journal、快照和 changeset (author 一律 `tracefs`); reset / revert / 进程合并 / 反向更新 / discard 都调它.
- `store::Undo`: 一串失败时倒序执行的清理 (删子卷、卸载、删行), 成功后 `commit()` 丢掉.
- `callers::pidfd_of`: 按 pid 和启动时间 (`/proc/<pid>/stat` 第 22 项) 认回一个进程, 用于 `end_on_exit` 的 session 和重启后的 agent 进程.

## 汇报与提交

- 汇报时把「已验证 / 未运行 / 受环境限制 / 未做真机验证」分开写.
- 提交信息用英文, Conventional Commits, 每行不超过 72 字符. 第一行 `<目录>: <做了什么>` 或 `<类型>(<目录>): <做了什么>`,
  正文写原因和验证情况. AI 参与的提交末尾加 `Co-Authored-By`.
- 暂存只用 `git add <路径>`, 不用 `git add .` / `-A` / `commit -a`.

## 设计

没实现的标 **(未做)**. 引用:
- "(验证 x.y)": `tests/kernel/probe.sh` 的检查项 (7.x 是 `tests/kernel/t07_perf.sh` 的基准), 测方案依赖的内核/btrfs/overlayfs 行为.
- "(测试 x.y)": `tests/tNN_*.sh` 的检查项, 测 tracefs 自身.
- "(早期 x.y)": 写代码前的验证, 脚本和原始输出已删除 (在 git 历史里).

### 1. 目标

提供类似 Mesa (https://www.mesa.dev/blog/introducing-mesa-filesystem-for-agents) 的能力, 不走 FUSE,
数据路径全部是内核原生文件系统.

只有一种模型:

- **工作区**: 注册时 tracefs 接管宿主目录, 把 workspace 的 `main` 挂在上面 (5.13). 之后谁直接改这个目录都一样 (用户的 vim,
  编译器), 改动停下后自动记成 changeset. 历史只追加, reset / revert / discard 也记成新的 changeset.
- **agent 进程** (`tracefs run -- <命令>`, 5.2): 以调用方身份跑在自己的 mount namespace 里, 每个工作区换成它自己的分支
  (btrfs 快照, CoW, O(1)); 它起的子进程 (bash, 编译器) 在同一个 namespace 里. 多个进程可以同时在同一组工作区上干活,
  互相看不到对方没合并的改动, 同一个文件各改各的副本. 一个进程可以同时改多个工作区.
- **合并在 task 干完之后**: 进程说一个 task 干完了 (`tracefs proc merge`, pi 扩展在一次 prompt 结束时调) 或进程退出时,
  它的分支合进 `main` (5.7), 再把 `main` 上别人的改动同步回它的分支. 中途的 tool call 只在它自己的分支上记 changeset.
  合并有冲突时交给第三个 agent (`--resolver`, 5.9) 智能合并; 合不了的留着, 进程状态 `conflicted`.
- **session** 只是给一组工作区 (多个 cwd) 的一段历史打的标签, 起止各对齐到一个快照, 可整段撤销或丢弃 (5.10).
  进程不属于 session: 它的 changeset 按每个工作区当时被哪个 session 标注记账, 所以一个进程可以同时改多个 session.

要求:

- 宿主目录的直接改动不漏记, 不依赖 harness 配合; 撤销经过挂载写回, 不打断目录里已打开的进程.
- 每个进程对每个工作区有独立 CoW 分支, 未合并的写入对其他进程不可见.
- 每次 tool call 结束可生成 **changeset**: 该进程所有分支的原子快照, 可查历史, diff, 回滚.
- task 干完时三方合并进 `main`, 合并跟随重命名 (5.7); 冲突交给 resolver, 再不行留给人.
- changeset 可增量推送到远端.

### 2. 非目标 (v1)

- write 级持久化和审计, 粒度是 changeset.
- 进程之间实时共享未合并的写入: 共享只通过 task 结束时的合并 (想要逐字节实时就得给文件加锁, fanotify 只拦得住 open,
  拦不住 delete / rename, 不完整).
- 运行中增加工作区 (之后注册的工作区会经挂载传播出现在进程里, 但那是 `main`, 不是它的分支).
- 远端按需懒加载 (第 9 节); 进程状态 fork (CRIU).
- 非 btrfs 原生支持 (用 loop btrfs 镜像兜底).
- 进程隔离: agent 以调用方身份跑, 和调用方看到同一个系统 (网络, home, 其他文件); 只有工作区换成了它自己的分支.
- base 里的硬链接: `index=off` 时 copy-up 会拆开硬链接 (验证 3.12).
- 同一步里既改名又以新文件保存 (`sed -i` 等先写临时文件再 rename) 的文件: 身份和内容都对不上, 合并按路径处理 (测试 11.4b).
- 宿主目录的直接改动按进程归因: 只记文件状态; harness 想标注就在 hook 里调 `tracefs changeset --author`.

### 3. 关键决策

| 项 | 决定 | 理由 |
| --- | --- | --- |
| 数据路径 | overlayfs | 接近原生性能, 无 FUSE 用户态往返 |
| 分支/快照 | btrfs subvolume 快照 | O(1), 块级 CoW, `btrfs send` 天然增量 |
| 层结构 | 单子卷布局: 一个分支子卷里同时放 base 各层 `l/1..l/n`, `upper/`, `work/`; upper 是自 base 以来的累积改动 | 每个子卷各自编 inode 号, 层分属不同子卷时视图里 `(st_dev, st_ino)` 重复, `find` 跳过目录, `cp -a` 漏掉 `.git` (验证 3.13, 早期 9.0); 同一子卷内唯一 (测试 2.5, 2.5b) |
| 分支层级 | `main` ← process 分支 (每个进程 × 工作区一个) | 并发进程互不干扰, 改动可归因到进程; 没有 session 分支: session 只是标签 |
| 进程 | `systemd-run --scope` 起 `tracefsd ns-helper run`: 新 mount namespace, 分支 bind 到工作区路径, 降到调用方身份后 exec | 不要容器镜像和 uid 映射, agent 用调用方的环境; scope 的 cgroup 用来冻结和收尾 |
| 工作区 | `main` 的 overlay 挂在宿主目录上; 原内容注册时移走删除 | 所有写入者都经过 overlay; 守护进程没起来时看到空目录, 而不是在过时的副本上改 |
| 合并时机 | task 干完 (`proc merge`) 和进程退出时, 不是每个 tool call | 用户要的语义: 同时改同一个文件不算错, 干完再合; 冲突交给第三个 agent |
| 改动检测 | inotify 监听 `main` 和运行中进程分支的 `upper/` 的所有目录, 去抖后快照, 与 head 无差异则丢弃 | 经 overlay 的写入全部落到 upper; upper 只含改过的目录, watch 数量小; fanotify 目录项事件要 FID 模式, 在 overlay 上未验证 |
| 撤销 | reset / revert / discard 都是一次合并 (5.7), 经挂载写入 (`main` 是宿主目录, 进程分支是它的 overlay), 结果记新 changeset | 不替换挂载中的子卷, 目录里的 shell, 编辑器, 运行中的 agent 不受影响 |
| 只读视图 | diff / merge / `fs --at` 读快照时临时挂一个只读 overlay (`run/views/`), 用完交给后台线程懒卸载 | 卸载一个视图 (含 `MNT_DETACH`) 实测 13~28 ms (内核等 RCU grace period), 一次 `fs write` 要卸 4 个; 挪出请求路径后 134 → 29 ms (ac) |
| 元数据连接 | 守护进程一直开着一个 SQLite 连接, 请求各开各的 | 最后一个连接关闭时 SQLite 做 checkpoint, 删 WAL 并 fsync, 每个请求多 3 次 fsync |
| 权限 | 守护进程以 root 持有所有 overlay 挂载; 进程的 namespace 里只是 bind | 才能用 `redirect_dir=on`, 避免 rootless 下目录 rename `EXDEV` (验证 3.18) |
| overlay 选项 | `redirect_dir=on,metacopy=off,index=off,xino=auto` | `metacopy=off` 简化 diff; btrfs 上 copy-up 走 reflink (验证 3.1) |
| 挂载 API | `fsopen/fsconfig/fsmount/move_mount`, `lowerdir+` | 无选项长度限制, 路径不用转义 (验证 2.2, 2.3, 2.10) |
| 工作区路径 | 进程里分支挂在与宿主相同的绝对路径 | 工具输出里的路径不用转换 |
| 文件身份 | 合并和 diff 按 inode 号认同一个条目, 只在同一子卷谱系内 (只经 `parent` 边相连的两个快照) 比较 | 单子卷布局里 copy-up 和 rename 都不改视图的 `st_ino`, 快照和 fork 也保留它 (验证 3.20); 不同 fork 各自新建的文件会撞号 (验证 3.20e). 不用自己分配 serial |
| diff v2 | `btrfs send --no-data -p A B` 列出改过的 upper 路径, 只比这些 | 内核只走两棵树不同的部分, 代价随改动量; 结果与 v1 一致 (测试里每次都对照) |
| 规则文件 | 工作区根目录的 `.tracefsignore` (gitignore 语法) 和 `.tracefsmerge` (`<模式> <策略>`), 模式用 `ignore` crate | 构建产物不进合并, 不触发自动 changeset; 锁文件等按规则决定, 不冲突 |
| 实现 | Rust: 守护进程 `tracefsd` (`crates/tracefs-daemon`), CLI `tracefs`, 共用 `tracefs-core`; SQLite 元数据 | 与 agentos-utils / agentos-runtime 一致 |
| IPC | Unix socket `/run/tracefs/tracefs-daemon.sock`, 每个连接一行 JSON 请求, 一行 JSON 响应 | 简单, SDK 好写 |
| 远端 | 一个目录 (本地盘, NFS, S3 挂载), 存 `btrfs send -p` 增量流 + `manifest.json` (sha256) | `btrfs receive` 直接恢复 (测试 6.6b) |

### 4. 数据模型

```
/var/lib/tracefs/              # 状态目录, 0700
├── meta.db
├── bases/<id>/             # ro 子卷: l/1 .. l/n (l/n 最上层)
├── branches/<id>/          # rw 子卷, 由 base 或 snap 快照而来: l/1..l/n, upper/, work/
├── snaps/<id>/             # ro 子卷, 分支某时刻的快照
└── tmp/                    # 启动时清空
/run/tracefs/                  # 私有 tmpfs (64M), 守护进程重启后还在, 重启机器后没有
├── tracefs-daemon.sock
├── mnt/<branch>/           # 运行中进程的分支 overlay (进程的 namespace 把它 bind 到工作区路径); 合并时临时挂的分支
├── views/                  # 只读快照视图 (diff, merge, `fs --at`), 压平用的挂载
└── resolve/<proc>/         # resolver 的工作目录 (5.9), 归进程的属主, 0700
```

meta.db 的表:

- `workspaces`: `name`, `host_path`, `uid/gid` (目录属主, workspace 归它), `main_branch`, `live` (detach 之后为 0).
- `bases`: `ws`, `layers`, `from_snap`.
- `sessions`: `name`, `state` (active / merged / discarded), `owner_uid`, `idle_timeout_ms`, `caller_pid`/`caller_start` (`end_on_exit` 绑定的进程).
  `session_live(session, ws, start_snap, end_snap)`: 标注的工作区, 一个工作区同时最多被一个活动 session 标注.
- `procs`: `state` (running / exiting (退出了, 正在合并) / conflicted / merged / discarded), `cmd`, `cwd`, `owner_uid/gid`, `pid`, `pid_start` (守护进程重启后据此认回进程).
  `proc_branches(proc, ws, branch, state)`: state 是 open (还有没合并的工作) / conflicted / merged / discarded.
- `branches`: `ws`, `base`, `kind` (main / process), `owner`, `parent_branch`, `head_snap`, `fork_snap` (求合并基点用), `alive`.
- `changesets`: `id` (ULID), `owner_kind/owner` (workspace 或 process; 合进 `main` 的合并归那个进程), `author`, `message`, `tool_call`, `session`.
- `snaps`: `branch`, `ws`, `base`, `changeset`, `parent`, `merge_parent`, `present`, `summary` (子卷删掉后保留 diff 摘要),
  `session` (拍快照时标注这个工作区的 session; 一个进程的 changeset 可以跨多个 session, `log --session` 按它查).
- `journal(branch, restore_snap, op)`: 改写分支前写入, 崩溃后据此恢复 (5.14).

旧版本的 `processes` / `proc_cwds` / `session_branches` 表 (容器模式) 启动时删掉, 它们的分支标成不活, 子卷当孤儿清掉.

不变量:

1. `bases/*` `snaps/*` 一律设 btrfs 只读标志. 分支里的 `l/*` 不是 ro 子卷, 只靠守护进程不写; 进程只通过 overlay 看到它们.
2. 一个 upper 同时最多被一个 overlay 挂载. 内核在 `index=off` 时不拦 (只报 dmesg, 验证 2.6), 由每分支的进程内锁和挂载检查 (statx `MOUNT_ROOT`) 保证.
3. process 分支只挂在 `run/mnt/<branch>` (进程的 namespace 里是它的 bind); `main` 始终挂在 `host_path`, 合并直接写这个挂载.
4. 同一目标分支的合并串行 (每分支一把锁).
5. snap 的 `upper/` 是自 base 以来的累积改动, 不是相对父 snap 的增量.
6. 一个 overlay 的所有层 (lower, upper, work) 在同一个子卷里.
7. 删除分支前先卸载: 删除挂载中的子卷不报错, overlay 会继续写到孤儿子卷上 (验证 4.5). 运行中进程的分支要删或替换子卷时先杀掉它的 scope.

### 5. 操作语义

#### 5.1 注册 workspace

0. 宿主目录 (解析符号链接后) 必须严格在调用方 home 下面, 不能是 home 本身, 不能和状态/运行目录重叠; 非 root 调用方还必须是目录属主.
   调用方 uid 取自 socket 的 `SO_PEERCRED`, home 查 `/etc/passwd`, 不信任请求里的字段. root 也只能用 `/root` 下面.
   `proc start` 对每个 cwd 再按调用方查一次. 测试用 `--workspace-root <dir>` 把 home 换成固定目录.
1. 建 `bases/<id>`, 宿主目录 `cp -ax --reflink=auto` 进 `l/1` (不进入下面的其他挂载), 设 ro.
2. `main` = `bases/<id>` 的可写快照, 建 `upper/` `work/`, 记一个 `register` changeset.
3. `l/1` 和每个分支 `upper/` 的根目录, 属主和权限照宿主目录 (overlay 根目录的属性取自 upper, 否则 agent 不能在工作区根下建文件).

#### 5.2 起进程 (`tracefs run`)

`tracefs [--socket S] run [--ws <ws>]... [--resolver <shell 命令>] -- <命令>`: CLI 把请求 (命令, 整个环境, 当前目录) 和自己的
stdin / stdout / stderr (`SCM_RIGHTS`) 交给守护进程, 之后这条连接一直开着.

1. 工作区: `--ws` 列出的, 默认调用方所有挂着的工作区. 每个先记下未记录的改动, 再从 `main` 的 head fork 一个 process 分支
   (5.4), 挂在 `run/mnt/<branch>`, 开始 inotify 监听它的 `upper/`.
2. `systemd-run --scope --unit <slice>-p<id>.scope --slice <slice>.slice -- /proc/<tracefsd>/exe ns-helper run`, 标准流是调用方的三个 fd,
   参数 (JSON) 经环境变量 `TRACEFS_RUN_SPEC` 给 (不进 argv: argv 谁都能读, 环境里有 API key). helper 以 root:
   `unshare(CLONE_NEWNS)`, `/` 递归设成 slave (之后宿主的新挂载照样传进来), 把每个分支 bind 到工作区路径,
   在 `run/mnt`, `run/views`, `run/resolve` 上盖空 tmpfs (别的分支看不到; socket 还在), 然后 setgroups (调用方 `/proc/<pid>/status`
   里的附加组), setresgid / setresuid 成调用方 (`SO_PEERCRED`), `setsid`, cd 到调用方的当前目录, 用调用方的环境 (加上
   `TRACEFS_PROC=<id>`) exec 命令. 进程号就是 systemd-run 的进程号, 也是进程组号.
3. 回第一行 `ProcStarted {id, pid, branches}`. CLI 收到 TERM / INT / HUP 时往连接里写 `{"signal": N}`, 守护进程转给进程组;
   连接断了 (CLI 被 SIGKILL) 守护进程给进程组 TERM, `--stop-timeout-ms` 后 `cgroup.kill` 整个 scope.
4. 进程退出: `cgroup.kill` scope 里剩下的进程, 停止监听, 每个分支打最后一个 changeset (`exit`), 卸载, 状态 `exiting`, 再按 5.8 合并 (带 resolver).
   回第二行 `ProcExited {id, code, signal, merge}`, CLI 以命令的退出码退出 (被信号杀: 128+N; tracefs 自己出错: 125), 有冲突时在 stderr 说.
5. 守护进程重启时进程还在 (在自己的 scope 里, 挂载在它的 namespace 里): 启动时按 `pid` + `pid_start` 认回仍在跑的进程,
   重新挂分支和监听, 等它退出后照常合并; 已经退出的直接合并; 合并到一半 (`exiting`) 的接着合并. 断了连接的 CLI 等进程退出,
   再等守护进程把它合并完 (`proc show` 的状态不再是 running / exiting), 以 1 退出 (退出码不知道了).

#### 5.3 自动 changeset

进程分支和 `main` 一样被 inotify 监听, 写入停顿 `--auto-quiet-ms` 或写满 `--auto-max-ms` 后拍一个只含这个分支的快照 (author `auto`),
与 head 无差异就删掉. harness 还可以在 tool call 前后显式记 (5.5).

#### 5.4 fork (内部原语)

`btrfs subvolume snapshot snaps/<s> branches/<new>` (连同 `l/` 和 `upper/`), 清空 `work/`,
`fork_snap = head_snap = s`. 不复制数据, 不增加层数.

#### 5.5 changeset (`proc changeset`, 每次 tool call 结束)

1. 冻结进程的 scope: 写 `cgroup.freeze` 并等 `cgroup.events` 报 frozen (进程已经冻结或不在跑就跳过).
2. 每个还有没合并工作的分支 `btrfs subvolume snapshot -r branches/<b> snaps/<new>`.
   快照会刷该子卷的脏数据, 包括没 msync 的 mmap 写入, 冻结期间不需要再 sync (测试 3.7b, 3.8).
3. 解冻. 结果带 `pause_ms` (只算冻结的这段).
4. 比较新快照和对照 (`--since <changeset>` 给了就是它在这个工作区的快照, 否则分支的 `head_snap`; 同 5.6, 去掉 `.tracefsignore` 的路径).
   所有分支都没变时删掉新快照, 不记 changeset, 结果 `changeset: null, unchanged: true` (和工作区的 `changeset` 一样).
5. 一个 SQLite 事务写 changeset 和 N 个 snap, 更新 `head_snap`.

不卸载, 不重启, 不影响其他进程. 目标暂停 < 100 ms (测试 3.7).

#### 5.6 diff (snap A → B, 同 workspace 同 base)

1. v2 (默认): `btrfs send --no-data -p A B` 的流里每个路径按流中先后的 rename 映射回 A 中的名字和 B 中的名字
   (新 inode 先以孤儿名 `o<ino>-<gen>-<n>` 出现, 目录改名带着下面的内容, 验证 4.9), `upper/` 下的都是候选, 父目录先比.
   v1: 并行遍历两个 `upper/`. 两者对每个名字的比较相同 (下面); 候选或遍历到的目录在两侧来源不同时整棵展开.
   `TRACEFS_DIFF_CHECK` 时 v2 每次都再跑一遍 v1, 不一致即报错 (集成测试一直开着); send 失败时退回 v1 并记日志.
   v1 的比较: 两侧都有的条目先比 `(ino, size, mtime, ctime, mode, uid, gid)`, 相同即未变;
   否则比属性, user xattr, 符号链接目标, 大小, 最后比两侧视图的 sha256 (同一个 lower 文件直接判相同).
   某个目录在两侧的 lower 来源不同 (opaque, redirect, whiteout, 类型不同) 时, 在两侧的只读视图里展开这个子树;
   其余地方只看 upper 里出现的条目 (与全量对比一致, 测试 3.1, 3.2).
2. 按 overlay 语义翻译: upper 中出现的条目, lower 有同名为修改, 否则为新增; whiteout 为删除;
   `trusted.overlay.opaque=y` 目录为整体替换; `trusted.overlay.redirect` 目录为重命名 (目录本身记 `rename`, 其下内容记新增).
3. 输出 `[{path, kind: add|modify|delete|rename, from?}]`. `--patch` 附 `diff -u` (超过 1 MiB 或含 NUL 的不出文本).
   A, B 同一谱系时, 按身份 (5.7) 认出的文件和目录改名也记 `rename` (随父目录移动的不单列).
4. `.tracefsignore` (B 的版本) 匹配的路径不输出, `--all` 时才列.

- changeset 的 diff = 每个 snap 对父 snap, 按 cwd 分组; 没有父 snap 时对空树.
- A, B 的 base 不同 (中间提升过 base) 或加 `--full` 时, 退化为两个只读视图的全量遍历.
- 子卷已删 (5.10, 5.13) 的 snap 返回保存的摘要, 不支持 `--patch`.

#### 5.7 merge (S → T, 内部原语)

1. 基点 O: 沿 `parent` / `merge_parent` 求最近公共 snap (只算子卷还在的; 首次合并即 S 的 `fork_snap`).
2. `diff(O, S)`, `diff(O, T)` 去掉 T 的 `.tracefsignore` 匹配的路径 (两边各留各的), 再认重命名:
   - 身份: 一侧与 O 同一谱系 (O 和它只经 `parent` 边相连) 时, O 中删掉或改过的条目与该侧新增或改过的条目按身份配对,
     身份不同路径的就是移动. 身份是视图里条目的 inode 号; 快照视图中 copy-up 过的条目报 upper 自己的 inode,
     原来的号在 `trusted.overlay.origin` 的 btrfs 文件句柄里 (验证 3.20f). 硬链接 (一个身份多个路径) 不跟随.
   - 内容: 剩下的删除和新增里, 内容相同且各自唯一的普通文件也算移动 (复制过, 或换了 inode); 不要求同一谱系.
   - 留在原路径的条目就是原来那个, 不管 inode 变没变 (编辑器保存时常换文件), 除非它原来的条目移走了.
3. 以对象为单位决定 (O 的每个条目, 和各侧新增的条目):
   - 结果路径: 一侧移动过就用那一侧的位置, 两侧移到不同位置记 `rename/rename`, 否则跟着父目录 (父目录可能被另一侧移动).
     互相移进对方记 `rename cycle`.
   - 删除: 另一侧没改过 (内容, 元数据, 相对父目录的位置) 才删, 否则记 `delete/modify` 或 `rename/delete`.
   - 内容: 只有一侧改了取那一侧. 两侧都改了: 先看 resolutions, 再看 `.tracefsmerge`, 再三方决定:
     一侧内容等于 O 就取另一侧的内容, 元数据取改过它的一侧 (`binary` 文件 chmod 对改内容也能合);
     两侧都改了文本 (三方前 8000 字节都无 NUL) 用 `git merge-file -p` 合并; 其余记冲突:
     合并失败, 二进制, add/add, 类型, 属性, 符号链接, 父目录被删或类型变了.
   - 两个条目落到同一结果路径: 两侧各自新增的按 add/add 决定, 有移动参与的记 `rename/add`.
4. `.tracefsmerge` 策略 (模式是 gitignore 语法, 最后匹配的一行生效, `!模式` 取消):
   `ours` 留 T 的, `theirs` 取 S 的, `newer` 取修改时间新的一侧 (删除按父目录的时间, 相同时取 T),
   `union` 文本冲突处两边的行都留, `unit=<名>` 同名的路径一起决定: 两侧都改过其中的路径时, 整组取改得最晚的一侧
   (包括另一侧改过而它没改的路径, 也恢复成它的版本).
5. 所有决定都在写入前算完; 有冲突则不写入, 返回冲突列表 (路径是结果路径), 调用方带 `resolutions` 重试
   (用不上的在结果的 `unused_resolutions` 里).
6. 无冲突: 持 T 的锁, 写 journal, **通过 T 的 overlay 挂载写入** (whiteout 由 overlay 生成, 不直接改 upper;
   路径用 `openat2(RESOLVE_BENEATH | NO_SYMLINKS | NO_XDEV)`): 要移动的条目先由深到浅改名到根下的临时名,
   再删除 (目录连同里面被忽略的文件一起删), 最后按结果路径从父到子放回移动的条目并写入
   (改名用 `RENAME_NOREPLACE`; overlay 的目录改名 `EXDEV` 时退化为复制). 再按 5.5 打 snap, `merge_parent = S`.
   整份取某一侧内容的普通文件不写数据: `merging.rs` `write_file` 的 `std::io::copy` 在两个 `File` 之间走 `copy_file_range`,
   从快照的只读视图到目标挂载, 两边都是 overlay, 内核转给底层的两个 btrfs 文件, 结果是 clone: 和快照共用 extent,
   不占新空间, 不写盘 (验证 4.7b, 测试 5.10; 10.187 上 reset 一个 1 GiB 文件 1.3 s, 结果和快照的物理 extent 相同,
   时间花在哪没细分). 所以那里的源和目标都要保持 `File`, 不要先读进内存. `FICLONE` 跨 overlay 仍是 `EXDEV` (验证 4.7).
   真正写数据的只有文本合并的结果和 resolution 给的内容; 移动只是改名.

#### 5.8 task 干完 (`proc merge`) 和进程退出

`tracefs proc merge <proc> [--resolutions <file>] [--discard-ws <ws>]... [--no-resolver]` (进程里用 `$TRACEFS_PROC`), 进程退出时守护进程自己做一遍:

1. 进程在跑: 冻结, 给所有还有没合并工作的分支打一个 changeset (`task done`), 解冻.
2. `--discard-ws`: 在跑的进程把那个分支经它的挂载写回 `main` 的内容 (5.7, O = 分支 head, S = `main`), 退出了的直接删分支.
3. 每个分支: 先记下 `main` 未记录的改动, 再按 5.7 合并进 `main` (O = 两个 head 的合并基点; 第一次是分支的 `fork_snap`,
   之后是上次合并进去的那个进程快照). 合并的 changeset 归这个进程, 记在 `main` 上, `merge_parent` 是进程快照.
4. 有冲突的分支什么都不写. 有 resolver 时交给它 (5.9), 拿它的决定连同 `--resolutions` 再合并一次; 还冲突的分支标 `conflicted`.
5. 进程在跑: 合并成功的分支, 冻结后打 `before update`, 再把 `main` 的新 head 合并进分支 (经 `run/mnt/<branch>` 写),
   进程接着在合了别人工作的分支上干. 冲突的分支不动, 下次 task 干完再试.
6. 进程退出了: 合并成功的分支删掉 (快照保留), 全部合并或丢弃则 `merged` (全丢弃为 `discarded`), 否则 `conflicted`,
   分支留着 (不挂载), 用 `proc merge --resolutions` 或 `--discard-ws` 收尾.

两个进程同时改了同一个文件不是错: 各自的分支里各是一份, 先干完的直接合进去, 后干完的那次合并才有冲突.

#### 5.9 resolver (第三个 agent)

`tracefs run --resolver <shell 命令>` (守护进程跑 `/bin/sh -c <命令>`). 只放在守护进程内存里 (连同进程的环境, 里面可能有 API key, 不写盘),
守护进程重启后没有 resolver, 冲突留给人.

1. 建 `run/resolve/<proc>/` (属主是进程的属主, 0700): `TASK.md` (给 agent 看的说明), `conflicts.json`
   (`[{id, ws, dir, path, reason, base, ours, theirs}]`), 每个冲突一个目录 `<id>/`, 里面 `base` (合并基点), `ours` (`main` 现在的),
   `theirs` (这个进程的) 三份普通文件 (那一侧不是普通文件或超过 4 MiB 时为 null, 总共最多 32 MiB).
2. 以进程的属主 (uid / gid, 没有附加组) 和进程的环境 (不带 `TRACEFS_PROC`) 在这个目录里跑 resolver (加 `TRACEFS_RESOLVE_DIR`), 自己的进程组,
   输出写 `log`, 最长 `--resolver-timeout-secs` (默认 1800), 超时杀进程组, 冲突留着.
3. 读 `resolutions.json`: `[{id, action: ours|theirs|content}]`, `content` 取 `<id>/result` (64 MiB 以内). 读这些文件用
   `openat2(RESOLVE_BENEATH | NO_SYMLINKS)`, resolver 用符号链接指到别处也读不到. 没写的 id 仍是冲突.
4. 结果的 `resolver` 字段说它做了什么 (`resolved 2 of 3 conflicts (exit 0)`, 超时, 失败原因). 都合并了就删掉这个目录.

#### 5.10 session

- `session create [--name n] [--ws w]... [--idle-timeout 秒]`: 先记下每个工作区未记录的改动, 以当时的 head 为 `start_snap`.
  之后这些工作区上的所有 changeset (自动, 手动, 进程合并进来的) 和进程分支上的快照都带这个 session (`snaps.session`).
- `session merge <s>`: 先记下未记录的改动, 写 `end_snap`, 标签结束, 内容不动.
- `session discard <s>`: 同上, 再把每个工作区恢复到 `start_snap` (5.13 的 discard).
- 自动结束 (merge): `--idle-timeout` 内没有带该 session 的快照且无未记录写入; `end_on_exit` 的调用方进程退出; 机器重启后结束所有 session.

#### 5.11 回滚

`reset <changeset|snap>` 和 `revert` 作用在快照所在的分支上: 快照属于一个还在跑的进程的分支, 就写那个分支 (先冻结进程, 打一个
`before ...` 的快照, 经 `run/mnt/<branch>` 写, 解冻); 否则写工作区的 `main` (经宿主目录). 都是 5.7 的合并, 结果记新 changeset.

进程分支上的 reset 只撤这个进程自己的改动: 从 head 沿 `parent` 往回走到目标快照, 中间 `merge_parent` 在别的分支上的快照是从 `main` 同步进来的
(5.8 第 5 步), 不撤; 其余按段从新到旧各做一次 revert (O = 段的新端, S = 段的旧端). 否则回到同步之前的快照再合并时, 别人已经合进
`main` 的改动会被当成这个进程删掉的 (测试 5.8b). 目标不在 head 的 `parent` 链上 (例如 `/tree` 往前跳) 时退回整体 reset.
所以 harness 能撤销 agent 自己的 tool call (pi 扩展的 `/tracefs undo`, `/tree`), 也能事后撤销合进 `main` 的整次合并.

#### 5.12 提升 base 与压平

- `base promote <snap>`: 只能是 `main` 当前的 head. `bases/<new>` = 该 snap 的快照, `upper/` 改名为 `l/<n+1>`, 删 `work/`, 设 ro
  (upper 格式的目录可直接作 lower, 中间层的 whiteout/opaque/redirect 都生效, 测试 6.1d).
  `main` 换成新 base 上的新分支, 记 `promote base` changeset (内容不变). 已有 session 仍用旧 base.
- 层数会超过 `--max-layers` (默认 16) 时 promote 自动改为压平; 也可直接 `base flatten <ws>`:
  `bases/<new>` = head 的可写快照, `upper/` 改名为 `l/<n+1>`, 以 `l/<n+1>..l/1` 为 lower, 新的空 upper 挂载,
  对每一项用原值调 `utimensat` 触发 copy-up (`metacopy=off` 时是完整 copy-up, 走 reflink); 卸载后删掉 upper 里的
  whiteout 和 `trusted.overlay.*`, 删 `l/*`, upper 改名为 `l/1`, 设 ro (测试 6.3, 6.4b, 6.4c; 独占空间 0, 早期 4.7d).
  不能 `cp -a --reflink` 合并视图: overlay 文件到 btrfs 的 FICLONE 返回 `EXDEV` (验证 4.7).
- 挂着的工作区要换 main 的子卷, 必须先卸载宿主目录; 有进程在用 (`EBUSY`) 时拒绝并列出进程. 不懒卸载 (会违反不变量 2).

#### 5.13 工作区的 main

注册 (`ws register`): 按 5.1 拷进 base, 建 main, 然后接管宿主目录:

1. 不能和其他工作区重叠 (包含或被包含).
2. 在同一父目录建空目录 (属性照原目录), `renameat2(RENAME_EXCHANGE)` 与原目录交换, 把 main 的 overlay 挂到原路径.
   交换失败 (例如原路径本身是挂载点) 时原内容留在挂载下面, 只记警告.
3. 元数据提交后删掉换出来的原内容. 仍停在原目录里的进程作为 `stale_processes` 返回:
   它们的 cwd 变成已删除目录, 写入会报错, 而不是悄悄写到不被记录的地方; 重新 `cd` 即可.

自动 changeset:

- inotify 监听 main 分支 `upper/` 下的所有目录, 新建或移入的目录随即加监听; 事件队列溢出时所有工作区记为有改动.
  经 overlay 的写入 (新建, copy-up, whiteout, rename, chmod, xattr) 都有事件 (测试 VM 内核 7.2). mmap 写入没有事件, 由下一次事件或定期检查带上.
- 有事件后, 写入停顿 `--auto-quiet-ms` (默认 2 s) 或持续写满 `--auto-max-ms` (默认 30 s) 时快照, author/message 为 `auto`;
  与 head 无差异 (不算 `.tracefsignore` 匹配的路径) 就删掉. 守护进程自己经挂载写入 (合并, 撤销) 也会触发, 由这一步过滤.
  所以只在 `target/` 之类忽略的目录里写不记 changeset; 快照本身仍包含它们 (`diff --all` 可见).
- 每 10 × `auto-max-ms` 把所有工作区当作有改动检查一次; 启动时重新挂载并检查一次, 覆盖守护进程停止期间的写入
  (挂载不随守护进程退出). 重新挂载时挂载点下已有文件则警告 (它们被盖住, 没有丢).
- 手动: `tracefs changeset [<ws>...] [--session s] [--author a] [-m msg] [--tool-call id]`, 给 harness 的 hook 用
  (例如 Claude Code 的 PostToolUse); 全无改动时返回 `unchanged`.

撤销, 都按 5.7 合并, 经宿主挂载写入, 结果记为新 changeset (历史不删):

| 操作 | O (基点) | S (来源) | T (目标) | 效果 |
| --- | --- | --- | --- | --- |
| `tracefs reset <cs\|snap>` | 当前 head | 目标快照 | 当前 head | 内容等于目标; 不会冲突 |
| `tracefs revert <cs>` | cs 的快照 | 它的父快照 | 当前 head | 只撤销这一个 changeset; 后来改过同一处则冲突 |
| `tracefs revert --session s` | `end_snap` (进行中取当前 head) | `start_snap` | 当前 head | 只撤销 session 内的改动 |
| `tracefs revert --from a --to b` | b 的快照 | a 的快照 | 当前 head | 撤销 a 之后到 b 为止的所有 changeset (含自动快照), 如一轮 tool call |
| `session discard` | 当前 head | `start_snap` | 当前 head | 回到 session 开始时 |

每次写入前先记下未记录的改动, 让 T 与目录内容一致. 写入期间用户仍可能在改同一文件, 窗口很短, v1 接受.

历史稀疏化: 自动快照 (author `auto`) 在 `--retain-all-hours` (默认 24) 内全留, 之后到 `--retain-hourly-days` (默认 7) 每小时留最后一个,
再往后每天留最后一个 (UTC 日), 每 `--retain-interval-secs` (默认 600, 0 不做) 处理一次, 也可 `tracefs ws prune <ws> [--dry-run]`.
已经结束的进程的分支上的快照 (分支删了以后) 按同样的规则稀疏化.
不删: 手动记的 changeset (hook, 合并, 撤销), session 起止点, 活分支的 head 和 fork 点, journal 里的恢复点.
删掉的 snap 只删子卷, 留元数据和对父 snap 的 diff 摘要, 以它为父的 snap 也先存下摘要, 所以 `log` 和 `diff` 照常;
对它 `reset` / `revert` 则报错 (测试 t15).

其他操作:

- `push` 先记下未记录的改动; `pull` 到挂着的工作区拒绝 (先 detach); pull 新建的工作区不挂 (只有历史).
- `ws detach`: 记下未记录的改动, 把 head 的视图 `cp -a` 到旁边的临时目录, 非懒卸载 (在用则拒绝),
  `RENAME_EXCHANGE` 换进原路径, 删掉临时目录. 之后是普通目录, workspace 和历史保留; 有活动 session 标注,
  或有进程在它上面还有没合并的工作时拒绝.

#### 5.14 崩溃恢复

启动时按顺序: 卸载残留的视图; 重新挂载工作区; 按 journal 恢复改写到一半的分支 (`main` 不替换子卷,
把当时状态记成 `interrupted <op>; tracefs reset <snap> undoes it` 的 changeset, 由用户决定; 测试 7.18; 运行中进程的分支要替换子卷,
先杀掉进程的 scope); 卸载不属于运行中进程的分支; 清空 `run/resolve/`; 删孤儿子卷; 清空 `tmp/`; 认回运行中的进程 (5.2).
`meta.db` 不在而 `bases/` 有数据时拒绝启动.

#### 5.15 push / pull

远端目录 `<remote>/<ws 名>/`: `manifest.json` 加 `streams/<id>.btrfs` (base 和 main 的每个 snap, 相对父 snap 或 base 做 `btrfs send -p`).
push 只发新增的; pull 只做快进, 校验 sha256 后 `btrfs receive`, 每个 snap 记为 `<原 message> (pulled)` (不带 `merge_parent` 和 session).

#### 5.16 免挂载的文件访问

不进挂载就能读写工作区 (给只用文件工具的 harness):

- 读 (`fs.read` / `fs.list` / `fs.stat`): 默认读 `main` (直接读宿主目录的当前内容; detach 了的读 `main` 的 head),
  `--at` 读某个 changeset 或快照. 快照经只读视图读, 路径同样 `openat2`, 不跟随工作区里的符号链接.
- 写 (`fs.edit`): 一组操作 (write, mkdir, delete, rename, symlink, chmod) 在一个 changeset 里, 经宿主目录写 `main`. 新建的条目归工作区属主.
  `--base <changeset|snap>`: 操作碰到的路径 (本身, 上层或下层) 自那以后变过就不写, 返回 `conflict` (CLI 退出 2).
  写到一半出错时把当时的状态记成 changeset (同 5.14). 没有改动返回 `unchanged`, 不记录.

### 6. 进程的边界

- 身份: 调用方的 uid / gid / 附加组, 调用方的整个环境, 加 `TRACEFS_PROC`. 不是沙箱: 进程能做调用方能做的一切, 只是它看到的
  工作区是自己的分支. 没有 `--ws` 的工作区以外, 它写什么都直接落到宿主上.
- cgroup: `/<slice>.slice/<slice>-p<id>.scope` (systemd 建, `--collect`, 空了就回收); 冻结 (5.5) 和收尾 (`cgroup.kill`) 只碰这个目录.
  限额来自 `tracefs.slice` (`deploy/`).
- 收尾: 进程退出后 `cgroup.kill` 它留下的子进程 (后台任务), 再快照合并; 守护进程不替别人等后台任务.

### 7. 接口 (CLI 与 socket 请求一一对应)

```
tracefs ping
tracefs ws register <name> --from <dir> | list | detach <ws> | prune <ws> [--dry-run]
tracefs run [--ws <ws>]... [--resolver <shell 命令>] -- <cmd>...
tracefs proc changeset <proc> --author <a> -m <msg> [--tool-call <id>] [--since <changeset>]
tracefs proc merge <proc> [--resolutions <file>] [--discard-ws <ws>]... [--no-resolver]
tracefs proc kill <proc> [--signal N] | show <proc> | list
tracefs changeset [<ws>...] [--session <s>] [--author <a>] [-m <msg>] [--tool-call <id>]
tracefs reset <changeset|snap>
tracefs revert (<changeset> | --session <s> | --from <cs|snap> --to <cs|snap>) [--resolutions <file>]
tracefs session create [--name <n>] [--ws <ws>]... [--idle-timeout <秒>]
tracefs session list | merge <s> | discard <s>
tracefs log (--proc <p> | --session <s> | --ws <w>) [-n N]
tracefs diff <changeset|snap> [<changeset|snap>] [--patch] [--full] [--all]
tracefs push|pull <ws> [--remote <dir>]
tracefs base promote <snap> | flatten <ws>
tracefs fs cat|ls|stat <ws> [<path>] [--at <changeset|snap>]
tracefs fs write <ws> <path> [--file <f>] [--mode <八进制>] | rm <ws> <path>... | mkdir <ws> <path> | mv <ws> <from> <to>
tracefs fs ln <ws> <target> <path> | chmod <ws> <八进制> <path> | edit <ws> --ops <file>
        (fs 的写命令都可加 [--base <changeset|snap>] [--author <a>] [-m <msg>] [--tool-call <id>])
```

CLI 打印 JSON 结果 (`fs cat` 打印文件内容), 出错退出 1, 合并或 `fs` 写有冲突退出 2 (`run` 见 5.2). socket 用 `--socket` / `TRACEFS_SOCKET` 指定.
错误码: `invalid not_found state exists exhausted internal`.
`resolutions`: `[{ws, path, action: ours|theirs|content, content}]`, `content` 为 base64 (CLI 的文件里写 `content_path`).

请求和结果的类型定义在 `tracefs-core` (`proto.rs`, `results.rs`), 导出为 `schema/tracefs.schema.json`.
harness 不调 CLI, 直接连 socket: `sdk/typescript` (`@dopamine-org/tracefs`, GitHub Packages) 和 `sdk/python` (`tracefs-sdk`, 无依赖) 的类型由 schema 生成.
例外是 `proc.run`: 它要传标准流 (`SCM_RIGHTS`) 并有第二行响应, harness 起 agent 时跑 `tracefs run`.

鉴权: socket 是 `root:root 0666` (`--socket-mode`), 不设组; 守护进程对每个请求按 `SO_PEERCRED` 的 uid 判断. root 可以操作一切. 其他调用方:

- 工作区归目录属主; 注册的目录必须归调用方, 且在其 home 下面 (5.1).
- session 和进程归创建者; changeset 和快照按所在工作区判断. `tracefs run` 只能给调用方自己的工作区开分支, 进程以调用方身份跑.
- 别人的对象一律报 `not_found`, 列表只返回自己的.
- `push`/`pull` 只能用 `--remote-dir`, 不能自选 `--remote`; pull 来的工作区路径按 5.1 检查, 属主必须是调用方.
- 每个调用方最多 `--max-user-sessions` (默认 16) 个活动 session; 内存和进程数由 `tracefs.slice` 限制 (`deploy/`).

调用约定: 用户 `ws register` 一次; harness 用 `tracefs run [--resolver ...] -- <agent>` 起 agent, agent (或它的扩展) 在每个 tool call
前后 `proc changeset --author <harness> -m <摘要> --tool-call <id>`, 一个 task 干完时 `proc merge $TRACEFS_PROC`; pi 扩展 (`sdk/pi`)
这样做: 一轮里有非只读工具 (`read` / `grep` / `find` / `ls` 以外) 时, 调用前后各取一次 changeset (撤销用 `revert --from <调用前> --to <调用后>`),
都带 `--since <上一个存档点>`: 调用前和上一个存档点一样就不记, 直接拿上一个存档点当「调用前」; 调用后和「调用前」一样 (只读的 bash 这类)
也不记, 这一轮就没有存档点. 拿「调用前」而不是 `head_snap` 比, 是因为这一轮里的改动可能已经被自动快照拍走了. prompt 结束 (`agent_settled`) 时合并.
想把一段工作整体标注: `session create --ws <w>... [--idle-timeout N]`, 结束时 `session merge` 或 `session discard`;
事后用 `revert --session` 撤销整段. harness 崩溃时: SDK 的 `session.create` 可带 `end_on_exit: true`, 守护进程用 `SO_PEERPIDFD`
(Linux 6.5) 拿发起请求的进程的 pidfd, 进程退出后按空闲超时同样的规则结束这个 session.

### 8. 性能指标

早期 x86_64 NVMe 测试机 (CachyOS btrfs, 已停用) 上的原型实测, 对照原生 btrfs 目录与 fuse-overlayfs (内核层基准, 现在是 `tests/kernel/t07_perf.sh`; 原始输出在 git 历史里).

| 指标 | 目标 | 实测 |
| --- | --- | --- |
| 读密集 (`git status` 10 万文件, `rg` 全仓) | 不慢于原生 10% | `rg` +4%; `git status` 热缓存 +17~20%, 冷缓存 +14%, 未达标 |
| 写密集 (`cargo build` 中型 crate, fio 4k 随机写) | 不慢于原生 15% | 与原生持平 |
| changeset 暂停 (3 cwd, 1 万文件改动内) | < 100 ms, 其他进程不暂停 | 41~56 ms (loop); 其他进程最大停顿 0.7 ms |
| diff (1 千文件变化, 10 万文件仓库) | v1 < 2 s, v2 < 200 ms | v1 原型 91 ms; v2 `send` 部分 44 ms |
| 8 进程同一工作区改不同文件后依次合并 | 全部无冲突, 结果为改动之并 | 通过 (测试 4.1; ac 上 8 个同时退出, 全部合完 771 ms) |
| fuse-overlayfs 对照 | 读写都显著更快 | `rg` 6.3x, 冷缓存 `git status` 87x, buffered 写 1.7x |

实现本身 (`tests/bench.sh`, ac: x86_64, 16 CPU, 62 GiB, 内核 7.2, 状态目录在 loop btrfs 镜像上). 同一天先后跑的两份, 机器负载有波动,
差 20~30% 以内的不算数:

| 操作 | 38769e3 | 0490d46 (视图后台卸载, 常驻元数据连接) |
| --- | --- | --- |
| 注册 5 万个文件的目录 | 2510 ms | 2389 ms |
| `tracefs run` 到命令开始, 3 个工作区 | 325 ms | 170 ms |
| proc changeset, 1 万个文件改动 (暂停) | 82 ms | 91 ms |
| diff 这个 1 万文件的 changeset (v2 / v1) | 949 / 416 ms | 928 / 279 ms |
| 其后一个 10 文件的 changeset 的 diff (v2 / v1) | 147 / 267 ms | 17 / 92 ms |
| proc merge: 1 万个改动的文件合进 main | 3466 ms | 2807 ms |
| proc merge: 一个改了名的 2500 文件目录 | 476 ms | 154 ms |
| 记下 5000 处改动后, 再写 1 处的 changeset (v2 / v1) | 99 / 234 ms | 28 / 88 ms |
| fs cat (50 次平均, 含 CLI 进程) | 3.2 ms | 3.6 ms |
| fs cat 一个记录过的状态 | 20.9 ms | 3.6 ms |
| fs write (50 次平均) | 152 ms | 37 ms |

diff v2 的代价随改动量, v1 随累积的 upper: 小改动 (每次 tool call, 自动快照) v2 快几倍;
upper 里的条目几乎全都变了时 v2 (主要是 btrfs send 本身) 约是 v1 的 3 倍.

### 9. 功能与测试

| 功能 | 测试 |
| --- | --- |
| 存储核心: btrfs/overlay 封装, meta.db, 注册, 分支层级, fork, changeset, 回滚 | t01, t02, t03, t05 |
| diff/merge: diff v1, merge, 合并进 main 和反向更新, resolutions | t03, t04 |
| 进程: `tracefs run` (mount namespace + scope), task 干完再合并, resolver | t02, t05 |
| 性能: diff v2, 基准 | 内核层 `tests/kernel/t07_perf.sh`, 实现 `tests/bench.sh` (第 8 节) |
| 远端: `btrfs send -p` 增量流 + manifest 推到远端目录, 校验和 | t06 |
| 工作区 (5.13), 快照保留策略, 忽略规则 | t07, t12, t15 |
| 多用户: 第 7 节鉴权, `deploy/` | t09 |
| 合并跟随重命名 (身份 + 内容), `.tracefsmerge` 规则 | t04, t11, t12, t14 |
| 免挂载读写 (5.16) | t13 |
| SDK: schema 导出, TS / Python SDK, `end_on_exit` 绑定进程 | t08, t10 |

**(未做)**: base 改 EROFS 镜像 + fscache on-demand 懒加载 (可选, 评估 composefs); detach 后重新 attach.

### 10. 风险与对策

| 风险 | 对策 |
| --- | --- |
| 挂载期间误改 lower | `bases/*` `snaps/*` 为 ro 子卷; 分支内 `l/*` 只靠守护进程不写; 状态目录 0700 |
| upper 重复挂载, 删除挂载中的分支 | 内核不拦; 每分支一把锁, 挂载前查挂载状态, 删除前先卸载 (不变量 2, 7) |
| 同文件并发修改导致冲突多 | task 干完才合, 冲突交给 resolver; task 切小, 合并后进程拿到 `main` 的新内容 |
| changeset 暂停随改动量增长 | 快照本身 O(1), 成本在刷该子卷脏数据 |
| 快照膨胀 (进程 × 工作区 × tool call) | main 的自动快照和结束了的进程的快照按时间稀疏化 (5.13), 只留元数据和 diff 摘要 |
| 认错重命名 | 身份只在同一谱系内比较, 一个身份多个路径不跟随; 按内容认只用于各自唯一的相同内容. 认不出时退回按路径合并 |
| send 流格式变化 | 解析失败或 send 失败时退回 v1 并记日志; 测试里退回即失败 |
| overlay 元数据开销: `git status` 慢 17~20% (落到用户日常操作) | 推荐开 `core.untrackedCache` / `core.fsmonitor`; 可 `ws detach`; 仍不达标时评估纯 btrfs 方案 (分支子卷直接 bind mount, 早期 t08) |
| 目录跨父目录移动, 绝对路径超过 `redirect_max` (默认 256) 时 `rename` 返回 `EXDEV` | `mv` 退化为复制; 必要时调大 overlay 模块参数 `redirect_max` (宿主全局) |
| merge 大文件是完整复制 | 移动的文件和目录只改名; 改了内容的仍是完整复制, 以后可在 T 未挂载时直接 reflink 进 upper (需自己生成 whiteout/opaque) |
| agent 以调用方身份跑, 不是沙箱 | 隔离的只有工作区 (第 6 节); 要防恶意 agent 得另起容器或虚拟机 |
| resolver 拿到进程的环境 (API key) | 只在守护进程内存里, 不写盘; resolver 以进程属主身份跑, 只能读写它的 resolve 目录 (和属主本来能碰的) |
| 非 btrfs 主机 | loop btrfs 镜像, buffered 随机写约 −45% (测于 loop 未开 direct I/O 时; 现在开了, 未重测) |
| 守护进程没起来时工作区目录是空的 | 守护进程退出不卸载, 只有重启机器后才需要它重新挂载 (`tracefsd.service` 开机启动); 用户会话若用私有 mount namespace 需确认传播 (未验证) |
| 撤销写入时用户同时改同一文件 | 写入前先记下未记录的改动, 窗口很短; 冲突的 revert 不写入 |
| 自动快照拍下编译到一半的产物; 快照随时间累积 | `.tracefsignore` 里的路径不触发也不进 diff 和合并; 自动快照按时间稀疏化; upper 增长用 `base promote` 压成新层 |
