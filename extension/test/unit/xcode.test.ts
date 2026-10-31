// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { compareXcodeVersions, findXcodeXips, isXipPath, xcodeVersionOf, type XipFs } from "../../src/setup/xcode";

function fakeFs(files: Record<string, { size: number; mtimeMs: number; dir?: boolean }>): XipFs {
  return {
    readdir: async () => Object.keys(files),
    stat: async (file) => {
      const f = files[path.basename(file)];
      if (!f) {
        throw new Error("ENOENT");
      }
      return { size: f.size, mtimeMs: f.mtimeMs, isFile: () => !f.dir };
    },
  };
}

describe("Xcode archive detection", () => {
  it("reads the version from the file name", () => {
    expect(xcodeVersionOf("Xcode_27.2.xip")).toBe("27.2");
    expect(xcodeVersionOf("Xcode_27_beta_3.xip")).toBe("27 beta 3");
    expect(xcodeVersionOf("Xcode.xip")).toBeUndefined();
    expect(xcodeVersionOf("Xcode_27.2.zip")).toBeUndefined();
  });

  it("orders newest Xcode first, releases before betas", () => {
    const sorted = ["26.4", "27.2 beta 1", "27.10", "27.2", undefined].sort(compareXcodeVersions);
    expect(sorted).toEqual(["27.10", "27.2", "27.2 beta 1", "26.4", undefined]);
  });

  it("lists only Xcode_*.xip files in Downloads", async () => {
    const found = await findXcodeXips("C:\\Users\\someone\\Downloads", fakeFs({
      "Xcode_26.4.xip": { size: 3e9, mtimeMs: 1 },
      "Xcode_27.2.xip": { size: 3.4e9, mtimeMs: 2 },
      "Xcode_27.2.xip.part": { size: 1, mtimeMs: 3 },
      "notes.txt": { size: 1, mtimeMs: 4 },
      "Xcode_folder.xip": { size: 0, mtimeMs: 5, dir: true },
    }));
    expect(found.map((f) => [f.fileName, f.version])).toEqual([["Xcode_27.2.xip", "27.2"], ["Xcode_26.4.xip", "26.4"]]);
    expect(await findXcodeXips("missing", { readdir: async () => { throw new Error("ENOENT"); }, stat: async () => { throw new Error(); } })).toEqual([]);
  });

  it("accepts only absolute .xip paths for the runner", () => {
    expect(isXipPath("C:\\Users\\someone\\Downloads\\Xcode_27.2.xip")).toBe(true);
    expect(isXipPath("Xcode_27.2.xip")).toBe(false);
    expect(isXipPath("C:\\x\\Xcode.dmg")).toBe(false);
    expect(isXipPath('C:\\x\\"evil.xip')).toBe(false);
  });
});
