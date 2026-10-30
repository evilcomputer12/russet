// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

import { describe, it, expect, afterEach } from "vitest";
import * as path from "node:path";
import { defaultSocketPath, russetBinary } from "../../src/daemon/locate";

const saved = { ...process.env };
afterEach(() => {
  process.env = { ...saved };
});

describe("defaultSocketPath", () => {
  it("honours an explicit configured path", () => {
    expect(defaultSocketPath("/tmp/custom.sock")).toBe("/tmp/custom.sock");
  });

  it("uses RUSSET_HOME/runtime/russetd.sock when set", () => {
    process.env.RUSSET_HOME = "/home/x/wt/.russet-dev";
    delete process.env.XDG_RUNTIME_DIR;
    expect(defaultSocketPath("")).toBe(
      path.join("/home/x/wt/.russet-dev", "runtime", "russetd.sock"),
    );
  });

  it("falls back to XDG_RUNTIME_DIR/russet/russetd.sock", () => {
    delete process.env.RUSSET_HOME;
    process.env.XDG_RUNTIME_DIR = "/run/user/1000";
    expect(defaultSocketPath("")).toBe(
      path.join("/run/user/1000", "russet", "russetd.sock"),
    );
  });
});

describe("russetBinary", () => {
  it("returns a bare name when no binDir is set", () => {
    const name = process.platform === "win32" ? "russet.exe" : "russet";
    expect(russetBinary("")).toBe(name);
  });

  it("joins the configured binDir", () => {
    const name = process.platform === "win32" ? "russet.exe" : "russet";
    expect(russetBinary("/opt/russet/bin")).toBe(path.join("/opt/russet/bin", name));
  });
});
