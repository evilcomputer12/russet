// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Russet updating itself on Windows: a release installation (installation.json "type":
// "release") checks GitHub's `windows-v*` releases every few hours, offers a newer one, and
// runs its installer silently, which closes Russet, updates it in place and starts it again.
// The installer is checked against the SHA-256 the release publishes beside it.

import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { createWriteStream, promises as fs } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import { log } from "../util/log";

export const RELEASES_URL = "https://api.github.com/repos/evilcomputer12/russet/releases?per_page=20";
const TAG = /^windows-v(\d+)\.(\d+)\.(\d+)$/;
const INSTALLER = /^RussetSetup-[0-9.]+\.exe$/;
const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;

export interface Release {
  version: string;
  installer: string;
  checksum?: string;
  page: string;
}

interface GitHubRelease {
  tag_name?: string;
  draft?: boolean;
  prerelease?: boolean;
  html_url?: string;
  assets?: { name?: string; browser_download_url?: string }[];
}

const parts = (v: string): number[] => v.split(".").map(Number);

export function newer(a: string, b: string): boolean {
  const [x, y] = [parts(a), parts(b)];
  for (let i = 0; i < 3; i++) {
    if ((x[i] ?? 0) !== (y[i] ?? 0)) return (x[i] ?? 0) > (y[i] ?? 0);
  }
  return false;
}

/** The newest published Windows release above `current`, with its installer, if any. */
export function pickUpdate(current: string, releases: readonly GitHubRelease[]): Release | undefined {
  let best: Release | undefined;
  for (const r of releases) {
    const m = TAG.exec(r.tag_name ?? "");
    if (!m || r.draft || r.prerelease) continue;
    const version = `${m[1]}.${m[2]}.${m[3]}`;
    const installer = r.assets?.find((a) => INSTALLER.test(a.name ?? ""));
    if (!installer?.browser_download_url || !newer(version, current) || (best && !newer(version, best.version))) continue;
    const checksum = r.assets?.find((a) => a.name === `${installer.name}.sha256`)?.browser_download_url;
    best = { version, installer: installer.browser_download_url, ...(checksum ? { checksum } : {}), page: r.html_url ?? "" };
  }
  return best;
}

async function installation(): Promise<{ root: string; version: string } | undefined> {
  const root = process.env.RUSSET_INSTALL_ROOT;
  if (!root) return undefined;
  try {
    const record = JSON.parse(await fs.readFile(path.join(root, "installation.json"), "utf8")) as { type?: string; version?: string };
    return record.type === "release" && typeof record.version === "string" ? { root, version: record.version } : undefined;
  } catch {
    return undefined;
  }
}

async function download(url: string, file: string, progress: (fraction: number) => void): Promise<string> {
  const response = await fetch(url);
  if (!response.ok || !response.body) throw new Error(`download failed (${response.status})`);
  const total = Number(response.headers.get("content-length") ?? 0);
  const hash = createHash("sha256");
  const out = createWriteStream(file);
  let done = 0;
  for await (const chunk of response.body as unknown as AsyncIterable<Uint8Array>) {
    hash.update(chunk);
    out.write(chunk);
    done += chunk.length;
    if (total) progress(done / total);
  }
  await new Promise<void>((resolve, reject) => out.end((err?: Error | null) => (err ? reject(err) : resolve())));
  return hash.digest("hex");
}

export function registerSelfUpdate(context: vscode.ExtensionContext): void {
  let checking = false;
  const check = async (manual: boolean): Promise<void> => {
    if (checking) return;
    checking = true;
    try {
      const install = await installation();
      if (!install) {
        if (manual) void vscode.window.showInformationMessage("This is a development installation of Russet; it updates from source.");
        return;
      }
      const response = await fetch(RELEASES_URL, { headers: { accept: "application/vnd.github+json" } });
      if (!response.ok) throw new Error(`GitHub answered ${response.status}`);
      const update = pickUpdate(install.version, (await response.json()) as GitHubRelease[]);
      if (!update) {
        if (manual) void vscode.window.showInformationMessage(`Russet ${install.version} is the latest release.`);
        return;
      }
      if (!manual && context.globalState.get<string>("russet.selfUpdate.skipped") === update.version) return;
      const pick = await vscode.window.showInformationMessage(
        `Russet ${update.version} is available (you have ${install.version}).`,
        "Update and Restart",
        "Release Notes",
        "Skip This Version",
      );
      if (pick === "Release Notes") {
        void vscode.env.openExternal(vscode.Uri.parse(update.page));
      } else if (pick === "Skip This Version") {
        await context.globalState.update("russet.selfUpdate.skipped", update.version);
      } else if (pick === "Update and Restart") {
        await install_(update);
      }
    } catch (err) {
      log().warn(`[update] Russet release check: ${(err as Error).message}`);
      if (manual) void vscode.window.showWarningMessage(`Russet could not check for updates: ${(err as Error).message}`);
    } finally {
      checking = false;
    }
  };

  const install_ = async (update: Release): Promise<void> => {
    const file = path.join(os.tmpdir(), `RussetSetup-${update.version}.exe`);
    const ok = await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Notification, title: `Downloading Russet ${update.version}` },
      async (progress) => {
        let last = 0;
        const digest = await download(update.installer, file, (f) => {
          progress.report({ increment: (f - last) * 100 });
          last = f;
        });
        if (update.checksum) {
          const expected = (await (await fetch(update.checksum)).text()).trim().split(/\s+/)[0]?.toLowerCase();
          if (expected !== digest) {
            await fs.rm(file, { force: true });
            void vscode.window.showErrorMessage("The downloaded Russet installer does not match its published checksum; nothing was installed.");
            return false;
          }
        }
        return true;
      },
    );
    if (!ok) return;
    // The installer closes Russet, replaces it and starts it again (windows/installer/russet.iss, /UPDATE=1).
    spawn(file, ["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS", "/UPDATE=1"], { detached: true, stdio: "ignore" }).unref();
    log().info(`[update] installing Russet ${update.version}`);
  };

  context.subscriptions.push(vscode.commands.registerCommand("russet.update.check", () => check(true)));
  const first = setTimeout(() => void check(false), 60_000);
  const timer = setInterval(() => void check(false), CHECK_EVERY_MS);
  context.subscriptions.push({ dispose: () => (clearTimeout(first), clearInterval(timer)) });
}
