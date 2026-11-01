// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, expect, it } from "vitest";
import {
  encodeBinaryPlist,
  isBinaryPlist,
  parseBinaryPlist,
  parseXmlPlist,
  PlistError,
  PlistReal,
  PlistUid,
  profileXml,
  toXmlPlist,
  type PlistValue,
} from "../../src/apple/plist";

// A synthetic plist written by Python's plistlib (FMT_BINARY), an independent encoder.
const PLISTLIB =
  "YnBsaXN0MDDeAQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGR4fIVNCaWdUQmxvYlVCdWlsdF8QEkNGQnVuZGxlSWRlbnRpZmllcl8QD0NGQnVuZGxlVmVyc2lvblVDb3VudF8QEkxTUmVxdWlyZXNJUGhvbmVPU1RMb25nVE5hbWVYTmVnYXRpdmVWTmVzdGVkVVJhdGlvXxAcVUlSZXF1aXJlZERldmljZUNhcGFiaWxpdGllc18QEVVJU3RhdHVzQmFySGlkZGVuEwAAAQAAAAAASQAB/lJ1c3NldDNByEuwoAAAAF8QEWRldi5ydXNzZXQuc2FtcGxlUTEQKglfECh4eHh4eHh4eHh4eHh4eHh4eHh4eHh4eHh4eHh4eHh4eHh4eHh4eHh4ZgBDAGEAZgDpACAmAxP/////////+dIaGxwdVUVtcHR5VUlubmVyoNAjP+AAAAAAAAChIFVhcm02NAgACAAlACkALgA0AEkAWwBhAHYAewCAAIkAkACWALUAyQDSANwA5QD5APsA/QD+ASkBNgE/AUQBSgFQAVEBUgFbAV0BYwAAAAAAAAIBAAAAAAAAACIAAAAAAAAAAAAAAAAAAAFk";

const bytes = (b64: string): Uint8Array => Uint8Array.from(Buffer.from(b64, "base64"));
const dict = (v: PlistValue): Map<string, PlistValue> => v as Map<string, PlistValue>;

describe("binary property lists", () => {
  it("reads every type plistlib writes", () => {
    const data = bytes(PLISTLIB);
    expect(isBinaryPlist(data)).toBe(true);
    const v = dict(parseBinaryPlist(data));
    expect(v.get("CFBundleIdentifier")).toBe("dev.russet.sample");
    expect(v.get("UIRequiredDeviceCapabilities")).toEqual(["arm64"]);
    expect([v.get("LSRequiresIPhoneOS"), v.get("UIStatusBarHidden")]).toEqual([true, false]);
    expect([v.get("Count"), v.get("Big"), v.get("Negative")]).toEqual([42, 2 ** 40, -7]);
    expect(v.get("Ratio")).toEqual(new PlistReal(0.5));
    expect((v.get("Built") as Date).toISOString()).toBe("2026-11-01T12:00:00.000Z");
    expect(Buffer.from(v.get("Blob") as Uint8Array).toString("latin1")).toBe("\x00\x01\xfeRusset");
    expect(v.get("Name")).toBe("Café ☃");
    expect(v.get("Long")).toBe("x".repeat(40));
    expect(dict(v.get("Nested")!).get("Empty")).toEqual([]);
  });

  it("writes what it reads, through binary and through XML", () => {
    const original = parseBinaryPlist(bytes(PLISTLIB));
    expect(parseBinaryPlist(encodeBinaryPlist(original))).toEqual(original);
    const xml = toXmlPlist(original);
    expect(xml).toContain("<key>CFBundleIdentifier</key>\n\t<string>dev.russet.sample</string>");
    expect(xml).toContain("<date>2026-11-01T12:00:00Z</date>");
    expect(xml).toContain("<array/>");
    expect(parseXmlPlist(xml)).toEqual(original);
  });

  it("keeps keyed-archive references, large tables and escapes", () => {
    const archive = new Map<string, PlistValue>([
      ["$top", new Map<string, PlistValue>([["root", new PlistUid(1)]])],
      ["$objects", Array.from({ length: 300 }, (_, i) => `object ${i} <&>`)],
    ]);
    const round = parseBinaryPlist(encodeBinaryPlist(archive));
    expect(round).toEqual(archive);
    expect(parseXmlPlist(toXmlPlist(archive))).toEqual(archive);
  });

  it("refuses damaged input", () => {
    expect(() => parseBinaryPlist(new TextEncoder().encode("bplist00 too short"))).toThrow(PlistError);
    expect(() => parseXmlPlist("<plist><dict><string>no key</string></dict></plist>")).toThrow(PlistError);
  });

  it("finds the property list inside a provisioning profile", () => {
    const inner = toXmlPlist(new Map<string, PlistValue>([["Name", "Synthetic Profile"]]));
    const envelope = Uint8Array.from([0x30, 0x82, 0x01, ...new TextEncoder().encode(inner), 0xa0, 0x82]);
    expect(profileXml(envelope)?.trim()).toBe(inner.trim());
    expect(profileXml(new Uint8Array([1, 2, 3]))).toBeUndefined();
  });
});
