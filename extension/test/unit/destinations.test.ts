// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, expect, it } from "vitest";
import type { DeviceSummary } from "../../src/generated/protocol";
import { DestinationModel, mergeDestinations, type VirtualSource } from "../../src/panels/destinations";
import { parseInput } from "../../src/panels/virtualIphonePanel";
import { Store } from "../../src/state/store";
import { DEFAULT_VIRTUAL, isValidChoice, pairingsFor, virtualDevices } from "../../src/setup/virtualCatalog";
import { rotate, toDevicePoint } from "../../webview/shared/geometry";

const phone = (udid: string, state: DeviceSummary["state"] = "ready"): DeviceSummary => ({
  udid, name: "Synthetic Phone", productType: "iPhone15,2", marketingName: "iPhone 14 Pro", osVersion: "26.1", transport: "usb", state,
});

function memento() {
  const data = new Map<string, unknown>();
  return { get: <T>(k: string) => data.get(k) as T | undefined, update: async (k: string, v: unknown) => void data.set(k, v), keys: () => [...data.keys()] };
}

describe("run destinations", () => {
  const created: VirtualSource = { created: true, choice: { device: "iPhone17,3", ios: "27.0.1" } };

  it("lists physical iPhones first and the runner-created virtual iPhone after them", () => {
    const all = mergeDestinations([phone("00000000-SYNTHETIC-B", "needs-trust"), phone("00000000-SYNTHETIC-A")], created);
    expect(all.map((d) => [d.kind, d.ready, d.stateLabel])).toEqual([
      ["physical", true, "Ready"],
      ["physical", false, "Needs trust"],
      ["virtual", false, "Not running"],
    ]);
    expect(all[2]).toMatchObject({ id: "virtual:iPhone17,3:27.0.1", name: "iPhone 16 Pro", osVersion: "27.0.1", transport: "virtual" });
    expect(mergeDestinations([], { ...created, created: false })).toEqual([]);
  });

  it("slots in daemon devices with transport virtual unchanged", () => {
    const vm = { ...phone("00000000-SYNTHETIC-VM"), productType: "iPhone17,3", marketingName: undefined, osVersion: "27.0.1", transport: "virtual" as never };
    const all = mergeDestinations([vm], created);
    expect(all).toHaveLength(1);
    expect(all[0]).toMatchObject({ kind: "virtual", name: "iPhone 16 Pro", ready: true, transport: "virtual", device: vm });
  });

  it("selects a virtual iPhone, blocks Run on it until the provider exists, and returns to a phone", () => {
    const store = new Store(memento() as never);
    store.setDevices([phone("00000000-SYNTHETIC-A")]);
    const model = new DestinationModel(store, memento() as never, () => created);
    expect(model.selected?.kind).toBe("physical");
    expect(model.runBlocker()).toBeUndefined();
    model.select("virtual:iPhone17,3:27.0.1");
    expect(model.selected).toMatchObject({ kind: "virtual", name: "iPhone 16 Pro" });
    expect(model.runBlocker()).toMatch(/virtual device provider/);
    model.select("00000000-SYNTHETIC-A");
    expect(model.selected?.kind).toBe("physical");
    expect(store.activeDevice?.udid).toBe("00000000-SYNTHETIC-A");
    model.dispose();
    store.dispose();
  });
});

describe("virtual iPhone catalog", () => {
  it("defaults to iPhone17,3 with iOS 27.0.1, a Linux-tested pairing", () => {
    expect(DEFAULT_VIRTUAL).toEqual({ device: "iPhone17,3", ios: "27.0.1" });
    expect(pairingsFor("iPhone17,3").find((p) => p.ios === "27.0.1")?.linuxTested).toBe(true);
    expect(isValidChoice(DEFAULT_VIRTUAL)).toBe(true);
    expect(isValidChoice({ device: "iPhone17,3", ios: "1.0" })).toBe(false);
    for (const d of virtualDevices()) {
      expect(pairingsFor(d.id).length).toBeGreaterThan(0);
    }
  });
});

describe("virtual iPhone input", () => {
  it("maps taps to device coordinates within 2 px at three probe points in every orientation", () => {
    // A 1206 x 2622 panel shown at 300 x 652 (portrait) or 652 x 300 (landscape).
    const W = 1206;
    const H = 2622;
    const probes = [
      { x: 0.1, y: 0.1 },
      { x: 0.5, y: 0.5 },
      { x: 0.9, y: 0.8 },
    ];
    for (const o of [0, 90, 180, 270] as const) {
      const land = o === 90 || o === 270;
      const w = land ? 652 : 300;
      const h = land ? 300 : 652;
      for (const p of probes) {
        // Where the device point appears on screen for this orientation.
        const screen =
          o === 0 ? { u: p.x, v: p.y } : o === 90 ? { u: 1 - p.y, v: p.x } : o === 180 ? { u: 1 - p.x, v: 1 - p.y } : { u: p.y, v: 1 - p.x };
        const back = toDevicePoint(screen.u * w, screen.v * h, w, h, o);
        expect(Math.abs(back.x - p.x) * W).toBeLessThan(2);
        expect(Math.abs(back.y - p.y) * H).toBeLessThan(2);
      }
    }
    expect(toDevicePoint(-5, 9999, 300, 652, 0)).toEqual({ x: 0, y: 1 });
  });

  it("rotates in quarter turns", () => {
    expect(rotate(0, "right")).toBe(90);
    expect(rotate(0, "left")).toBe(270);
    expect(rotate(270, "right")).toBe(0);
  });

  it("accepts only well-formed input events from the page", () => {
    expect(parseInput({ type: "touch", phase: "down", x: 0.5, y: 0.25 })).toEqual({ type: "touch", phase: "down", x: 0.5, y: 0.25 });
    expect(parseInput({ type: "touch", phase: "down", x: 2, y: 0 })).toBeUndefined();
    expect(parseInput({ type: "key", key: "a", code: "KeyA", shift: true })).toEqual({ type: "key", key: "a", code: "KeyA", shift: true });
    expect(parseInput({ type: "key", key: "x".repeat(100), code: "KeyA" })).toBeUndefined();
    expect(parseInput({ type: "button", button: "home" })).toEqual({ type: "button", button: "home" });
    expect(parseInput({ type: "button", button: "selfDestruct" })).toBeUndefined();
    expect(parseInput("nope")).toBeUndefined();
  });
});
