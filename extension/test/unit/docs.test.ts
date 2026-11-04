// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, it, expect } from "vitest";
import { errorPage, slug, windowsPaths } from "../../src/util/docs";
import { readable } from "../../src/run/buildLog";

describe("Russet's bundled documentation", () => {
  it("opens an error's docs at its heading in the bundled error reference", () => {
    expect(errorPage("docs/reference/errors.md#build.compileFailed")).toEqual({ page: "errors.md", heading: "build.compileFailed" });
    expect(slug("build.compileFailed")).toBe("buildcompilefailed");
    expect(slug("Run your app on an iPhone")).toBe("run-your-app-on-an-iphone");
    expect(errorPage("https://example.invalid/x")).toBeUndefined();
  });

  it("shows the backend's Windows-drive paths as Windows paths, on Windows only", () => {
    const line = "/mnt/c/Users/someone/Desktop/Test/Sources/Test/TestApp.swift:10:13: error: cannot find 'ContentView' in scope";
    expect(windowsPaths(line, "win32")).toBe(
      "C:\\Users\\someone\\Desktop\\Test\\Sources\\Test\\TestApp.swift:10:13: error: cannot find 'ContentView' in scope",
    );
    expect(windowsPaths("the full log is /mnt/d/p/.russet/logs/job_1.jsonl.", "win32")).toBe(
      "the full log is D:\\p\\.russet\\logs\\job_1.jsonl.",
    );
    expect(windowsPaths("/home/russet/.russet/ide/x and /mnt/c/x", "linux")).toBe("/home/russet/.russet/ide/x and /mnt/c/x");
  });

  it("cuts a line too long to read short in the build output", () => {
    const command = `/usr/lib/swift/usr/bin/swift-frontend ${"-x ".repeat(1000)}`;
    const shown = readable(command, "win32");
    expect(shown.length).toBeLessThan(320);
    expect(shown).toMatch(/more characters, in the build's log file\)$/);
    expect(readable("short", "win32")).toBe("short");
  });
});
