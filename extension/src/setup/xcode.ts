// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Finds the Xcode archive the user downloaded from Apple (`Xcode_*.xip` in
// Downloads) for the `xcode-sdk` setup step. Detection is the page's job; the
// runner receives the chosen path as `--arg xip=<Windows path>`.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

export interface XipCandidate {
  path: string;
  fileName: string;
  /** "27.2", "27.0 beta 3", or undefined when the name carries no version. */
  version?: string;
  sizeBytes: number;
  mtimeMs: number;
}

export interface XipFs {
  readdir(dir: string): Promise<string[]>;
  stat(file: string): Promise<{ size: number; mtimeMs: number; isFile(): boolean }>;
}

const nodeFs: XipFs = {
  readdir: (dir) => fs.promises.readdir(dir),
  stat: (file) => fs.promises.stat(file),
};

const XIP_NAME = /^Xcode[_ -]?(.*)\.xip$/i;

export function downloadsDir(env: NodeJS.ProcessEnv = process.env): string {
  const base = env.USERPROFILE || os.homedir();
  return path.join(base, "Downloads");
}

/** "Xcode_27.2.xip" → "27.2"; "Xcode_27_beta_3.xip" → "27 beta 3". */
export function xcodeVersionOf(fileName: string): string | undefined {
  const m = XIP_NAME.exec(fileName);
  if (!m) {
    return undefined;
  }
  const v = m[1].replace(/_/g, " ").trim();
  return /^\d/.test(v) ? v : undefined;
}

/** Numeric-aware descending order: 27.10 > 27.2 > 27.2 beta 1 > 26.4. */
export function compareXcodeVersions(a: string | undefined, b: string | undefined): number {
  if (a === b) {
    return 0;
  }
  if (!a) {
    return 1;
  }
  if (!b) {
    return -1;
  }
  const parse = (v: string): { nums: number[]; pre: number } => {
    const [main, ...rest] = v.split(/\s+/);
    const nums = main.split(".").map((n) => Number.parseInt(n, 10) || 0);
    const preNum = rest.length ? Number.parseInt(rest[rest.length - 1], 10) : Number.NaN;
    return { nums, pre: rest.length === 0 ? Number.POSITIVE_INFINITY : Number.isNaN(preNum) ? 0 : preNum };
  };
  const pa = parse(a);
  const pb = parse(b);
  for (let i = 0; i < Math.max(pa.nums.length, pb.nums.length); i++) {
    const d = (pb.nums[i] ?? 0) - (pa.nums[i] ?? 0);
    if (d !== 0) {
      return d;
    }
  }
  return pb.pre === pa.pre ? 0 : pb.pre > pa.pre ? 1 : -1;
}

/** Every `Xcode_*.xip` in `dir`, newest Xcode first. A missing folder yields []. */
export async function findXcodeXips(dir: string, fsApi: XipFs = nodeFs): Promise<XipCandidate[]> {
  let names: string[];
  try {
    names = await fsApi.readdir(dir);
  } catch {
    return [];
  }
  const found: XipCandidate[] = [];
  for (const fileName of names) {
    if (!XIP_NAME.test(fileName)) {
      continue;
    }
    const file = path.join(dir, fileName);
    try {
      const st = await fsApi.stat(file);
      if (!st.isFile()) {
        continue;
      }
      found.push({ path: file, fileName, version: xcodeVersionOf(fileName), sizeBytes: st.size, mtimeMs: st.mtimeMs });
    } catch {
      /* unreadable entries are skipped */
    }
  }
  return found.sort((a, b) => compareXcodeVersions(a.version, b.version) || b.mtimeMs - a.mtimeMs);
}

/** A path the runner may receive: absolute and ending in `.xip`. */
export function isXipPath(p: string): boolean {
  return (path.isAbsolute(p) || path.win32.isAbsolute(p)) && /\.xip$/i.test(p) && !/["\r\n]/.test(p);
}

export function formatBytes(n: number): string {
  if (n >= 1024 ** 3) {
    return `${(n / 1024 ** 3).toFixed(1)} GB`;
  }
  if (n >= 1024 ** 2) {
    return `${Math.round(n / 1024 ** 2)} MB`;
  }
  return `${Math.max(1, Math.round(n / 1024))} KB`;
}
