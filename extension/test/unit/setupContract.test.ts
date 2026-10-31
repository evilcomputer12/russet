// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, expect, it } from "vitest";
import { cleanText, parseRunEvent, parseStatus, runArgv, SetupProtocolError } from "../../src/setup/contract";

// The live runner's status line on 2026-10-31, as printed by Russet.Host.exe.
const LIVE =
  '{"version":1,"steps":[{"id":"wsl","state":"done","title":"Windows Subsystem for Linux","detail":"Installed."},' +
  '{"id":"backend","state":"done","title":"Russet backend","detail":"Ready."},' +
  '{"id":"apple-devices","state":"done","title":"Apple Devices (USB)","detail":"Apple Devices is running."},' +
  '{"id":"iphone","state":"unavailable","title":"iPhone","detail":"Checked in the device picker once the backend is ready."},' +
  '{"id":"xtool","state":"done","title":"xtool","detail":"Installed."},' +
  '{"id":"xcode-sdk","state":"needed","title":"iOS SDK from Xcode","detail":"Choose an Xcode .xip to build the iOS SDK."},' +
  '{"id":"vphone","state":"needed","title":"Virtual iPhone engine","detail":"Not built yet (about an hour)."},' +
  '{"id":"virtual-iphone","state":"unavailable","title":"Virtual iPhone","detail":"Create a virtual iPhone (several hours on first boot)."}]}';

describe("setup runner contract", () => {
  it("parses the live runner's status line", () => {
    const s = parseStatus(LIVE);
    expect(s.steps.map((x) => x.id)).toEqual([
      "wsl", "backend", "apple-devices", "iphone", "xtool", "xcode-sdk", "vphone", "virtual-iphone",
    ]);
    expect(s.steps[3]).toEqual({
      id: "iphone", state: "unavailable", title: "iPhone",
      detail: "Checked in the device picker once the backend is ready.",
    });
  });

  it("rejects other versions and non-JSON, and tolerates unknown states and ids", () => {
    expect(() => parseStatus('{"version":2,"steps":[]}')).toThrow(SetupProtocolError);
    expect(() => parseStatus("not json")).toThrow(SetupProtocolError);
    const s = parseStatus(JSON.stringify({
      version: 1,
      steps: [
        { id: "future-step", state: "warming-up", title: "Future" },
        { id: "wsl", state: "done", title: "WSL" },
        { id: "wsl", state: "needed", title: "dup" },
        { id: "../evil", state: "done", title: "x" },
        { id: "backend", state: "done", title: "B\u0000ad\u001btitle" },
      ],
    }));
    expect(s.steps.map((x) => [x.id, x.state])).toEqual([
      ["future-step", "unavailable"], ["wsl", "done"], ["backend", "done"],
    ]);
    expect(s.steps[2].title).toBe("B ad title");
  });

  it("parses run events and clamps progress", () => {
    expect(parseRunEvent('{"step":"vphone","event":"progress","message":"Building","fraction":1.7}', "vphone"))
      .toEqual({ step: "vphone", event: "progress", message: "Building", fraction: 1 });
    expect(parseRunEvent('{"step":"vphone","event":"progress","message":"Building"}', "vphone"))
      .toEqual({ step: "vphone", event: "progress", message: "Building" });
    expect(parseRunEvent("   ", "vphone")).toBeUndefined();
    expect(parseRunEvent('{"step":"vphone","event":"done"}', "vphone")).toEqual({ step: "vphone", event: "done" });
    expect(parseRunEvent('{"step":"vphone","event":"failed","code":"nope","message":"x"}', "vphone")).toEqual({
      step: "vphone", event: "failed", code: "setup.vphone.failed", message: "x", retryable: true,
    });
    expect(parseRunEvent('{"step":"vphone","event":"failed","code":"setup.vphone.disk","message":"Full","retryable":false}', "vphone"))
      .toMatchObject({ code: "setup.vphone.disk", retryable: false });
    expect(() => parseRunEvent('{"step":"wsl","event":"done"}', "vphone")).toThrow(/another step/);
    expect(() => parseRunEvent('{"step":"vphone","event":"explode"}', "vphone")).toThrow(/unknown event/);
  });

  it("builds argv and refuses unsafe arguments", () => {
    expect(runArgv("xcode-sdk", { xip: "C:\\Users\\someone\\Downloads\\Xcode_27.2.xip" })).toEqual([
      "--setup", "run", "xcode-sdk", "--arg", "xip=C:\\Users\\someone\\Downloads\\Xcode_27.2.xip",
    ]);
    expect(runArgv("virtual-iphone", { device: "iPhone17,3", ios: "27.0.1" })).toEqual([
      "--setup", "run", "virtual-iphone", "--arg", "device=iPhone17,3", "--arg", "ios=27.0.1",
    ]);
    expect(() => runArgv("Bad Id")).toThrow();
    expect(() => runArgv("wsl", { "bad key": "x" })).toThrow();
    expect(() => runArgv("wsl", { k: 'a"b' })).toThrow();
    expect(() => runArgv("wsl", { k: "a\nb" })).toThrow();
  });

  it("cleans and bounds runner text", () => {
    expect(cleanText("a\u2028b\u0007c")).toBe("a b c");
    expect(cleanText("x".repeat(700)).length).toBe(600);
    expect(cleanText(42)).toBe("");
  });
});
