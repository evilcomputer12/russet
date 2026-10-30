// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Resolves the russetd socket path and the `russet` binary directory
// (ARCHITECTURE §7.1, §14). One daemon per RUSSET_HOME (worktree).

import * as os from "node:os";
import * as path from "node:path";
import * as crypto from "node:crypto";

/** The russetd Unix socket / Windows named pipe, following ARCHITECTURE §7.1. */
export function defaultSocketPath(configuredPath: string): string {
  if (configuredPath) {
    return configuredPath;
  }
  if (process.platform === "win32") {
    // \\.\pipe\russet-<user> (the real SID is resolved by the daemon; the user
    // name is a stable stand-in for the dev socket).
    const who = process.env.USERNAME ?? os.userInfo().username;
    return `\\\\.\\pipe\\russet-${who}`;
  }
  const home = process.env.RUSSET_HOME;
  if (home) {
    return path.join(home, "runtime", "russetd.sock");
  }
  const runtime = process.env.XDG_RUNTIME_DIR;
  if (runtime) {
    return path.join(runtime, "russet", "russetd.sock");
  }
  // Fallback: /tmp/russet-<uid>/[<hash>.sock] (ARCHITECTURE §7.1).
  const uid = typeof process.getuid === "function" ? process.getuid() : 0;
  const dir = path.join(os.tmpdir(), `russet-${uid}`);
  const full = path.join(dir, "russetd.sock");
  if (full.length <= 108) {
    return full;
  }
  const hash = crypto.createHash("sha256").update(full).digest("hex").slice(0, 16);
  return path.join(dir, `${hash}.sock`);
}

/** The directory that holds the `russet` CLI, from the setting or PATH. */
export function binDir(configuredBinDir: string): string {
  return configuredBinDir || "";
}

export function russetBinary(configuredBinDir: string): string {
  const name = process.platform === "win32" ? "russet.exe" : "russet";
  const dir = binDir(configuredBinDir);
  return dir ? path.join(dir, name) : name;
}
