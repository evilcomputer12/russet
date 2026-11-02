// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, expect, it } from "vitest";
import { parseRunEvent } from "../../src/setup/contract";
import { virtualDebugConfiguration, virtualDebugSession } from "../../src/setup/virtualDebug";

// The live runner's last event on 2026-11-01 (`Russet.Host.exe --setup run virtual-iphone-debug`),
// with a synthetic container id.
const APP = "/private/var/containers/Bundle/Application/00000000-0000-4000-8000-000000000000/Test.app";
const DONE = JSON.stringify({
  step: "virtual-iphone-debug",
  event: "done",
  result: {
    pid: 1804,
    executable: `${APP}/Test`,
    appDir: APP,
    gdbSocket: "/home/russet/.cache/russet/virtual-debug/1793532560/gdb.sock",
    dapSocket: "/home/russet/.cache/russet/virtual-debug/1793532560/dap.sock",
    localApp: "/mnt/c/Users/someone/My App/.russet/build/debug-iphoneos/Test.app",
  },
});

describe("Debug on the virtual iPhone", () => {
  it("carries the step's result on done", () => {
    const event = parseRunEvent(DONE, "virtual-iphone-debug");
    expect(event).toMatchObject({ event: "done", result: { pid: 1804, appDir: APP } });
    expect(parseRunEvent('{"step":"x","event":"done","result":[1]}', "x")).toEqual({ step: "x", event: "done" });
  });

  it("builds Russet's LLDB attach recipe from it", () => {
    const event = parseRunEvent(DONE, "virtual-iphone-debug");
    const session = virtualDebugSession(event?.event === "done" ? event.result : undefined);
    expect(session).toBeDefined();
    const config = virtualDebugConfiguration(session!);
    expect(config).toMatchObject({ type: "russet-virtual", request: "attach", __dapSocket: session!.dapSocket });
    expect(config.attachCommands).toEqual([
      "platform select remote-ios",
      'target create --arch arm64-apple-ios "/mnt/c/Users/someone/My App/.russet/build/debug-iphoneos/Test.app/Test"',
      `target modules search-paths add "${APP}" "/mnt/c/Users/someone/My App/.russet/build/debug-iphoneos/Test.app"`,
      "process connect -p gdb-remote unix-connect:///home/russet/.cache/russet/virtual-debug/1793532560/gdb.sock",
    ]);
  });

  it("refuses sockets outside the managed home and incomplete results", () => {
    const result = JSON.parse(DONE).result as Record<string, unknown>;
    expect(virtualDebugSession({ ...result, dapSocket: "/tmp/dap.sock" })).toBeUndefined();
    expect(virtualDebugSession({ ...result, gdbSocket: "/home/russet/../root/gdb.sock" })).toBeUndefined();
    expect(virtualDebugSession({ ...result, pid: "1804" })).toBeUndefined();
    expect(virtualDebugSession(undefined)).toBeUndefined();
  });
});
