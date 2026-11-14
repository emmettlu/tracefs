import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { Store } from "../index.js";

export const WIN = process.platform === "win32";

export function tmp(prefix = "tracefs-lite-test-") {
  return fs.realpathSync.native(fs.mkdtempSync(path.join(os.tmpdir(), prefix)));
}

export function fresh(t) {
  const root = tmp();
  t.after(() => fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 }));
  return Store.init(root);
}

export function write(dir, rel, text) {
  const f = path.join(dir, rel);
  fs.mkdirSync(path.dirname(f), { recursive: true });
  fs.writeFileSync(f, text);
}

export function read(dir, rel) {
  const f = path.join(dir, rel);
  return fs.existsSync(f) ? fs.readFileSync(f, "utf8") : undefined;
}

export function canSymlink(dir) {
  try {
    fs.symlinkSync("x", path.join(dir, ".probe"));
    fs.unlinkSync(path.join(dir, ".probe"));
    return true;
  } catch {
    return false;
  }
}
