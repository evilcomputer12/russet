// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

import { describe, it, expect, beforeEach } from "vitest";
import { Store } from "../../src/state/store";
import type { DeviceSummary, ProjectInfoResult } from "../../src/generated/protocol";

class FakeMemento {
  private map = new Map<string, unknown>();
  keys(): readonly string[] {
    return [...this.map.keys()];
  }
  get<T>(key: string, def?: T): T | undefined {
    return (this.map.has(key) ? (this.map.get(key) as T) : def) as T | undefined;
  }
  async update(key: string, value: unknown): Promise<void> {
    this.map.set(key, value);
  }
}

function device(udid: string, state: DeviceSummary["state"] = "ready"): DeviceSummary {
  return {
    udid,
    name: `iPhone ${udid}`,
    productType: "iPhone13,2",
    osVersion: "27.0",
    transport: "usb",
    state,
  };
}

describe("Store", () => {
  let store: Store;
  beforeEach(() => {
    store = new Store(new FakeMemento() as never);
  });

  it("selects the first ready device when none is active", () => {
    store.setDevices([device("A", "needs-trust"), device("B", "ready")]);
    expect(store.activeDevice?.udid).toBe("B");
  });

  it("keeps the active device across device-list refreshes", () => {
    store.setDevices([device("A"), device("B")]);
    store.setActiveDevice("B");
    store.setDevices([device("A"), device("B")]);
    expect(store.activeDevice?.udid).toBe("B");
  });

  it("falls back when the active device is unplugged", () => {
    store.setDevices([device("A"), device("B")]);
    store.setActiveDevice("B");
    store.setDevices([device("A")]);
    expect(store.activeDevice?.udid).toBe("A");
  });

  it("enables hot reload only for a running swiftpm/flutter session", () => {
    const project: ProjectInfoResult = {
      stack: "swiftpm",
      root: "/p",
      name: "App",
      targets: [{ name: "App", kind: "app" }],
      configurations: ["debug", "release"],
      defaultTarget: "App",
    };
    store.setProject(project);
    expect(store.canHotReload).toBe(false);
    store.setRun({ state: "running", isDebug: false });
    expect(store.canHotReload).toBe(true);
  });

  it("defaults the selected target to the project default", () => {
    const project: ProjectInfoResult = {
      stack: "swiftpm",
      root: "/p",
      name: "App",
      targets: [{ name: "App", kind: "app" }, { name: "Widget", kind: "app-extension" }],
      configurations: ["debug"],
      defaultTarget: "App",
    };
    store.setProject(project);
    expect(store.state.selectedTarget).toBe("App");
  });
});
