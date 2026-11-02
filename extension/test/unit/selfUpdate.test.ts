// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, expect, it, vi } from "vitest";

vi.mock("vscode", () => ({}));
const { newer, pickUpdate, planPatch } = await import("../../src/setup/selfUpdate");

const asset = (name: string) => ({ name, browser_download_url: `https://example.invalid/${name}` });

describe("Russet's own updates", () => {
  it("compares versions numerically", () => {
    expect(newer("0.10.0", "0.9.9")).toBe(true);
    expect(newer("0.6.0", "0.6.0")).toBe(false);
    expect(newer("1.0.0", "1.0.1")).toBe(false);
  });

  it("picks the newest published Windows release with an installer", () => {
    const releases = [
      { tag_name: "windows-v0.8.0", draft: true, assets: [asset("RussetSetup-0.8.0.exe")] },
      { tag_name: "windows-v0.7.1", prerelease: true, assets: [asset("RussetSetup-0.7.1.exe")] },
      { tag_name: "v0.9.0", assets: [asset("russet-linux.tar.gz")] },
      { tag_name: "windows-v0.7.0", html_url: "https://example.invalid/r/0.7.0",
        assets: [asset("RussetSetup-0.7.0.exe"), asset("RussetSetup-0.7.0.exe.sha256")] },
      { tag_name: "windows-v0.6.5", assets: [asset("RussetSetup-0.6.5.exe")] },
      { tag_name: "windows-v0.7.2", assets: [asset("notes.txt")] },
    ];
    const picked = pickUpdate("0.6.0", releases);
    expect(picked).toMatchObject({
      version: "0.7.0",
      installer: "https://example.invalid/RussetSetup-0.7.0.exe",
      checksum: "https://example.invalid/RussetSetup-0.7.0.exe.sha256",
      page: "https://example.invalid/r/0.7.0",
    });
    expect(picked?.manifest).toBeUndefined();
    expect(pickUpdate("0.7.0", releases)).toBeUndefined();
  });

  it("patches only the parts whose source changed, and falls back to the installer otherwise", () => {
    const sha = (c: string) => c.repeat(64);
    const manifest = {
      version: "0.6.1",
      editor: "ed1",
      components: {
        extension: { source: "ext2", asset: "russet-extension-0.6.1.zip", sha256: sha("a"), size: 140_000 },
        host: { source: "src1", asset: "russet-host-0.6.1.zip", sha256: sha("b"), size: 30_000_000 },
        launcher: { source: "src1", asset: "russet-launcher-0.6.1.zip", sha256: sha("c"), size: 30_000_000 },
        backend: { source: "be1", asset: "russet-backend-0.6.1.zip", sha256: sha("d"), size: 90_000_000 },
      },
    };
    const assets = Object.fromEntries(Object.values(manifest.components).map((c) => [c.asset, `https://example.invalid/${c.asset}`]));
    const installed = { extension: "ext1", host: "src1", launcher: "src1", backend: "be1", editor: "ed1" };
    expect(planPatch(installed, manifest, assets)).toEqual([
      { name: "extension", source: "ext2", url: "https://example.invalid/russet-extension-0.6.1.zip", sha256: sha("a"), size: 140_000 },
    ]);
    expect(planPatch({ ...installed, extension: "ext2" }, manifest, assets)).toEqual([]);
    // Another editor, an installation from before patches, or a package missing: the full installer.
    expect(planPatch({ ...installed, editor: "ed0" }, manifest, assets)).toBeUndefined();
    expect(planPatch(undefined, manifest, assets)).toBeUndefined();
    expect(planPatch(installed, manifest, {})).toBeUndefined();
  });
});
