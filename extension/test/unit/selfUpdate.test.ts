// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, expect, it, vi } from "vitest";

vi.mock("vscode", () => ({}));
const { newer, pickUpdate } = await import("../../src/setup/selfUpdate");

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
    expect(pickUpdate("0.6.0", releases)).toEqual({
      version: "0.7.0",
      installer: "https://example.invalid/RussetSetup-0.7.0.exe",
      checksum: "https://example.invalid/RussetSetup-0.7.0.exe.sha256",
      page: "https://example.invalid/r/0.7.0",
    });
    expect(pickUpdate("0.7.0", releases)).toBeUndefined();
  });
});
