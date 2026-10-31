// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

import { describe, it, expect } from "vitest";
import { configToWire } from "../../src/run/session";

describe("configToWire", () => {
  it("lowercases UI labels to the daemon's configuration names", () => {
    expect(configToWire("Debug")).toBe("debug");
    expect(configToWire("Release")).toBe("release");
  });
  it("defaults to debug", () => {
    expect(configToWire(undefined)).toBe("debug");
  });
});
