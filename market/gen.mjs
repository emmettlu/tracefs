import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..");
const commit = process.argv[2] ?? execFileSync("git", ["-C", repo, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
const git = (...a) => execFileSync("git", ["-C", repo, ...a], { encoding: "utf8" }).trim();
const pkg = JSON.parse(git("show", `${commit}:sdk/dsh/package.json`));
const prev = fs.existsSync(path.join(here, "index.json")) ? JSON.parse(fs.readFileSync(path.join(here, "index.json"), "utf8")) : null;
const old = prev?.plugins.find((p) => p.name === pkg.name)?.versions.filter((v) => v.version !== pkg.version) ?? [];
const version = {
  version: pkg.version,
  publishTimestamp: Number(git("show", "-s", "--format=%ct", commit)),
  source: { type: "git", url: "https://github.com/emmettlu/tracefs", commit, path: "sdk/dsh" },
  engines: { dsh: pkg.engines.dsh },
};
const index = {
  format: 1,
  title: { en: "tracefs", zh: "tracefs" },
  description: { en: "DSH bundles from tracefs (acfs)", zh: "tracefs (acfs) 的 DSH 插件" },
  metadata: { categories: { "developer-tools": { title: { en: "Developer tools", zh: "开发工具" } } } },
  plugins: [
    {
      name: pkg.name,
      title: { en: "acfs: undo for agents", zh: "acfs: 给 agent 的撤销" },
      description: {
        en: "Records every tool call that writes files as an acfs changeset, merges the work into main when a turn ends, and gives the model an acfs tool to list, diff and undo. acfsd on Linux, or an acfs-lite copy-on-write store on macOS (APFS image), Linux (btrfs) and Windows (NTFS VHDX).",
        zh: "把每次写文件的工具调用记成 acfs changeset, 回合结束时合进 main, 并给模型一个 acfs 工具来查看、diff 和撤销. Linux 上用 acfsd, 或在 macOS (APFS 镜像)、Linux (btrfs)、Windows (NTFS VHDX) 上用 acfs-lite 写时复制仓库.",
      },
      icon: `data:image/svg+xml;base64,${fs.readFileSync(path.join(here, "icon.svg")).toString("base64")}`,
      author: pkg.author,
      homepage: pkg.homepage,
      repository: { url: "https://github.com/emmettlu/tracefs", directory: "sdk/dsh" },
      metadata: { category: "developer-tools", keywords: pkg.keywords },
      "dist-tags": { latest: pkg.version },
      versions: [version, ...old],
    },
  ],
};
fs.writeFileSync(path.join(here, "index.json"), `${JSON.stringify(index, null, 2)}\n`);
console.log(`${pkg.name}@${pkg.version} -> ${commit}`);
