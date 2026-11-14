---
name: tracefs
description: List, diff, undo and merge the file changes recorded by tracefs. Use when the user asks what you changed, wants a change or a whole turn undone, wants to compare states, or wants a branch merged into main; also when a hook reports an tracefs merge conflict.
---

# tracefs

The tracefs plugin records every `Bash` and `apply_patch` call as a changeset (author `codex`, tagged with the tool call id) in a
copy-on-write store: tracefsd on Linux when Codex runs under `tracefs run`, otherwise an tracefs-lite store (an APFS image on macOS, btrfs or a
btrfs image on Linux, an NTFS VHDX on Windows). When a turn ends, work on a branch is merged into main.

Run the script from the directory you are working in (it finds the store from the current directory). `<skill>` is this skill's directory.

| Task | Command |
| --- | --- |
| Is tracefs active here, and where? | `node <skill>/scripts/tracefs.mjs status` |
| Recent changesets (id, author, tool call, message) | `node <skill>/scripts/tracefs.mjs log [n]` |
| What changed between two changesets, or since one | `node <skill>/scripts/tracefs.mjs diff <from> [to]` |
| Undo one changeset on top of later work | `node <skill>/scripts/tracefs.mjs undo <changeset>` |
| Merge this branch into main now | `node <skill>/scripts/tracefs.mjs merge` |

## How to work with it

- To undo "your last change", run `log`, take the newest changeset whose author is `codex` and whose message is not `before ...`, and `undo` it.
  To undo a whole turn, undo its changesets newest first.
- `undo` refuses (exit 2) when a path it would restore changed again later; report the listed paths instead of forcing it.
- `diff <first changeset of the conversation>` shows everything changed since the conversation began.
- A merge conflict (exit 2, or a hook message `tracefs: not merged into main yet`) names each path and why (`both modified`,
  `modified/deleted`, `both added`). Resolve it in the files, then run `merge` again.
- Undo and merge record changesets of their own, so they can be undone too.
- If `status` says tracefs is not active, tell the user how to start it rather than trying to set it up silently.
