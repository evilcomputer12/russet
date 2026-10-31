// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, expect, it } from "vitest";
import { issuerIdParam, issuerIdProblem } from "../../src/setup/ascKey";

// Synthetic: the shape of an App Store Connect issuer ID, not a real one.
const ISSUER = "6b0e4f1a-2c3d-4e5f-8a9b-0c1d2e3f4a5b";

describe("App Store Connect issuer ID", () => {
  it("accepts a UUID in either case and drops pasted whitespace", () => {
    expect(issuerIdProblem(ISSUER)).toBeUndefined();
    expect(issuerIdProblem(` ${ISSUER.toUpperCase()}\n`)).toBeUndefined();
    expect(issuerIdParam(` ${ISSUER}\n`)).toBe(ISSUER);
  });

  it("reads an empty answer as an individual key", () => {
    expect(issuerIdProblem("")).toBeUndefined();
    expect(issuerIdProblem("   ")).toBeUndefined();
    expect(issuerIdParam("   ")).toBeUndefined();
  });

  it("names the mix-up with a 10-character key or team ID", () => {
    expect(issuerIdProblem("SYNTHKEY01")).toMatch(/10-character key or team ID/);
    expect(issuerIdProblem(" synthteam1 ")).toMatch(/10-character key or team ID/);
  });

  it("rejects anything else that is not a dashed UUID", () => {
    for (const value of ["not-an-issuer", ISSUER.slice(1), `${ISSUER}0`, ISSUER.replaceAll("-", ""), `${ISSUER.slice(0, -1)}g`]) {
      expect(issuerIdProblem(value), value).toMatch(/is a UUID/);
    }
  });
});
