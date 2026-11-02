// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Property lists: Apple's binary format (bplist00) and its XML form, both ways, so a built app's
// binary Info.plist opens as XML in the editor and saves back as binary. No vscode import.

/** A real number (integers are plain numbers, or bigints beyond 2^53). */
export class PlistReal {
  constructor(readonly value: number) {}
}

/** A keyed archive's object reference (CF$UID). */
export class PlistUid {
  constructor(readonly value: number) {}
}

export type PlistValue =
  | string
  | boolean
  | number
  | bigint
  | PlistReal
  | PlistUid
  | Date
  | Uint8Array
  | PlistValue[]
  | Map<string, PlistValue>;

export class PlistError extends Error {}

/** Seconds between 1970-01-01 and Apple's reference date, 2001-01-01 (UTC). */
const APPLE_EPOCH = 978307200;

export function isBinaryPlist(bytes: Uint8Array): boolean {
  return bytes.length >= 8 && String.fromCharCode(...bytes.subarray(0, 6)) === "bplist";
}

// --- binary ---------------------------------------------------------------------------------

export function parseBinaryPlist(bytes: Uint8Array): PlistValue {
  if (!isBinaryPlist(bytes) || bytes.length < 40) {
    throw new PlistError("Not a binary property list.");
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const t = bytes.length - 32;
  const offsetSize = bytes[t + 6];
  const refSize = bytes[t + 7];
  const count = Number(view.getBigUint64(t + 8));
  const top = Number(view.getBigUint64(t + 16));
  const tableAt = Number(view.getBigUint64(t + 24));
  if (count > bytes.length || tableAt + count * offsetSize > t || top >= count) {
    throw new PlistError("The binary property list's trailer is damaged.");
  }
  const uint = (at: number, size: number): number => {
    let n = 0;
    for (let i = 0; i < size; i++) {
      n = n * 256 + bytes[at + i];
    }
    return n;
  };
  const offsets = Array.from({ length: count }, (_, i) => uint(tableAt + i * offsetSize, offsetSize));
  const active = new Set<number>();

  const read = (index: number): PlistValue => {
    if (active.has(index)) {
      throw new PlistError("The binary property list refers to itself.");
    }
    active.add(index);
    try {
      return object(offsets[index]);
    } finally {
      active.delete(index);
    }
  };
  // A length in the marker's low nibble, or an int object after it when the nibble is 0xF.
  const length = (at: number): [number, number] => {
    const low = bytes[at] & 0x0f;
    if (low !== 0x0f) {
      return [low, at + 1];
    }
    const size = 1 << (bytes[at + 1] & 0x0f);
    return [uint(at + 2, size), at + 2 + size];
  };
  const object = (at: number): PlistValue => {
    const marker = bytes[at];
    const kind = marker >> 4;
    const low = marker & 0x0f;
    switch (kind) {
      case 0x0:
        if (marker === 0x08) return false;
        if (marker === 0x09) return true;
        throw new PlistError(`Unsupported marker 0x${marker.toString(16)}.`);
      case 0x1: {
        const size = 1 << low;
        if (size === 8) {
          const n = view.getBigInt64(at + 1);
          return n >= BigInt(Number.MIN_SAFE_INTEGER) && n <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(n) : n;
        }
        if (size === 16) {
          // 128-bit integers hold unsigned 64-bit values in their low half.
          return view.getBigUint64(at + 9);
        }
        return uint(at + 1, size);
      }
      case 0x2:
        return new PlistReal(low === 2 ? view.getFloat32(at + 1) : view.getFloat64(at + 1));
      case 0x3:
        return new Date((view.getFloat64(at + 1) + APPLE_EPOCH) * 1000);
      case 0x4: {
        const [n, start] = length(at);
        return bytes.slice(start, start + n);
      }
      case 0x5: {
        const [n, start] = length(at);
        return String.fromCharCode(...bytes.subarray(start, start + n));
      }
      case 0x6: {
        const [n, start] = length(at);
        let s = "";
        for (let i = 0; i < n; i++) {
          s += String.fromCharCode(view.getUint16(start + i * 2));
        }
        return s;
      }
      case 0x8:
        return new PlistUid(uint(at + 1, low + 1));
      case 0xa: {
        const [n, start] = length(at);
        return Array.from({ length: n }, (_, i) => read(uint(start + i * refSize, refSize)));
      }
      case 0xd: {
        const [n, start] = length(at);
        const dict = new Map<string, PlistValue>();
        for (let i = 0; i < n; i++) {
          const key = read(uint(start + i * refSize, refSize));
          if (typeof key !== "string") {
            throw new PlistError("A dictionary key is not a string.");
          }
          dict.set(key, read(uint(start + (n + i) * refSize, refSize)));
        }
        return dict;
      }
      default:
        throw new PlistError(`Unsupported marker 0x${marker.toString(16)}.`);
    }
  };
  return read(top);
}

export function encodeBinaryPlist(root: PlistValue): Uint8Array {
  // Objects in order, each container's members after it; equal strings share one object.
  const objects: PlistValue[] = [];
  const strings = new Map<string, number>();
  const refs = new Map<number, number[]>();
  const add = (value: PlistValue): number => {
    if (typeof value === "string" && strings.has(value)) {
      return strings.get(value)!;
    }
    const index = objects.length;
    objects.push(value);
    if (typeof value === "string") {
      strings.set(value, index);
    } else if (Array.isArray(value)) {
      refs.set(index, value.map(add));
    } else if (value instanceof Map) {
      const keys = [...value.keys()].map(add);
      refs.set(index, [...keys, ...[...value.values()].map(add)]);
    }
    return index;
  };
  add(root);
  const refSize = objects.length < 256 ? 1 : objects.length < 65536 ? 2 : 4;
  const out: number[] = [...new TextEncoder().encode("bplist00")];
  const pushUint = (n: number, size: number): void => {
    for (let i = size - 1; i >= 0; i--) {
      out.push(Math.floor(n / 256 ** i) % 256);
    }
  };
  const pushInt = (n: number | bigint): void => {
    const big = BigInt(n);
    if (big >= 0n && big < 256n) {
      out.push(0x10, Number(big));
    } else if (big >= 0n && big < 65536n) {
      out.push(0x11);
      pushUint(Number(big), 2);
    } else if (big >= 0n && big < 4294967296n) {
      out.push(0x12);
      pushUint(Number(big), 4);
    } else {
      out.push(0x13);
      const b = new DataView(new ArrayBuffer(8));
      b.setBigInt64(0, BigInt.asIntN(64, big));
      out.push(...new Uint8Array(b.buffer));
    }
  };
  const marker = (kind: number, n: number): void => {
    if (n < 15) {
      out.push((kind << 4) | n);
    } else {
      out.push((kind << 4) | 0x0f);
      pushInt(n);
    }
  };
  const offsets: number[] = [];
  objects.forEach((value, index) => {
    offsets.push(out.length);
    if (typeof value === "boolean") {
      out.push(value ? 0x09 : 0x08);
    } else if (typeof value === "number" || typeof value === "bigint") {
      pushInt(value);
    } else if (value instanceof PlistReal) {
      const b = new DataView(new ArrayBuffer(8));
      b.setFloat64(0, value.value);
      out.push(0x23, ...new Uint8Array(b.buffer));
    } else if (value instanceof Date) {
      const b = new DataView(new ArrayBuffer(8));
      b.setFloat64(0, value.getTime() / 1000 - APPLE_EPOCH);
      out.push(0x33, ...new Uint8Array(b.buffer));
    } else if (value instanceof Uint8Array) {
      marker(0x4, value.length);
      out.push(...value);
    } else if (value instanceof PlistUid) {
      const size = value.value < 256 ? 1 : value.value < 65536 ? 2 : 4;
      out.push(0x80 | (size - 1));
      pushUint(value.value, size);
    } else if (typeof value === "string") {
      if ([...value].every((ch) => ch.charCodeAt(0) < 0x80)) {
        marker(0x5, value.length);
        for (let i = 0; i < value.length; i++) out.push(value.charCodeAt(i));
      } else {
        marker(0x6, value.length);
        for (let i = 0; i < value.length; i++) pushUint(value.charCodeAt(i), 2);
      }
    } else {
      const members = refs.get(index)!;
      marker(Array.isArray(value) ? 0xa : 0xd, Array.isArray(value) ? members.length : members.length / 2);
      for (const ref of members) pushUint(ref, refSize);
    }
  });
  const tableAt = out.length;
  const offsetSize = tableAt < 256 ? 1 : tableAt < 65536 ? 2 : tableAt < 4294967296 ? 4 : 8;
  for (const o of offsets) pushUint(o, offsetSize);
  out.push(0, 0, 0, 0, 0, 0, offsetSize, refSize);
  pushUint(objects.length, 8);
  pushUint(0, 8);
  pushUint(tableAt, 8);
  return Uint8Array.from(out);
}

// --- XML ------------------------------------------------------------------------------------

const escape = (s: string): string => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

function base64(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString("base64");
}

/** The XML form, as Xcode and plutil write it (tabs, wrapped data). */
export function toXmlPlist(root: PlistValue): string {
  const lines: string[] = [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">',
    '<plist version="1.0">',
  ];
  const emit = (value: PlistValue, depth: number): void => {
    const pad = "\t".repeat(depth);
    if (typeof value === "string") {
      lines.push(`${pad}<string>${escape(value)}</string>`);
    } else if (typeof value === "boolean") {
      lines.push(`${pad}<${value}/>`);
    } else if (typeof value === "number" || typeof value === "bigint") {
      lines.push(`${pad}<integer>${value}</integer>`);
    } else if (value instanceof PlistReal) {
      lines.push(`${pad}<real>${value.value}</real>`);
    } else if (value instanceof PlistUid) {
      lines.push(`${pad}<dict>`, `${pad}\t<key>CF$UID</key>`, `${pad}\t<integer>${value.value}</integer>`, `${pad}</dict>`);
    } else if (value instanceof Date) {
      lines.push(`${pad}<date>${value.toISOString().replace(/\.\d{3}Z$/, "Z")}</date>`);
    } else if (value instanceof Uint8Array) {
      lines.push(`${pad}<data>`);
      const text = base64(value);
      for (let i = 0; i < text.length; i += 68) {
        lines.push(`${pad}${text.slice(i, i + 68)}`);
      }
      lines.push(`${pad}</data>`);
    } else if (Array.isArray(value)) {
      if (value.length === 0) {
        lines.push(`${pad}<array/>`);
        return;
      }
      lines.push(`${pad}<array>`);
      value.forEach((v) => emit(v, depth + 1));
      lines.push(`${pad}</array>`);
    } else {
      if (value.size === 0) {
        lines.push(`${pad}<dict/>`);
        return;
      }
      lines.push(`${pad}<dict>`);
      for (const [k, v] of value) {
        lines.push(`${pad}\t<key>${escape(k)}</key>`);
        emit(v, depth + 1);
      }
      lines.push(`${pad}</dict>`);
    }
  };
  emit(root, 0);
  lines.push("</plist>", "");
  return lines.join("\n");
}

const ENTITIES: Record<string, string> = { lt: "<", gt: ">", amp: "&", quot: '"', apos: "'" };

function unescape(s: string): string {
  return s.replace(/&(#x[0-9a-fA-F]+|#\d+|lt|gt|amp|quot|apos);/g, (_, e: string) =>
    e.startsWith("#x") ? String.fromCodePoint(parseInt(e.slice(2), 16))
      : e.startsWith("#") ? String.fromCodePoint(parseInt(e.slice(1), 10))
        : ENTITIES[e],
  );
}

/** Parses the XML form (what the editor shows) back into values. */
export function parseXmlPlist(text: string): PlistValue {
  // Tags and the text between them; the prolog, doctype and comments are skipped.
  const body = text.replace(/<\?[\s\S]*?\?>|<!DOCTYPE[\s\S]*?>|<!--[\s\S]*?-->/g, "");
  const tokens = body.match(/<[^>]+>|[^<]+/g) ?? [];
  let i = 0;
  const next = (): string => {
    while (i < tokens.length && !tokens[i].startsWith("<") && tokens[i].trim() === "") i++;
    if (i >= tokens.length) throw new PlistError("The property list ends early.");
    return tokens[i++];
  };
  const textUntil = (close: string): string => {
    let s = "";
    while (i < tokens.length && tokens[i] !== close) {
      if (tokens[i].startsWith("<")) throw new PlistError(`Unexpected ${tokens[i]} before ${close}.`);
      s += tokens[i++];
    }
    if (i >= tokens.length) throw new PlistError(`Missing ${close}.`);
    i++;
    return unescape(s);
  };
  const value = (tag: string): PlistValue => {
    const name = tag.replace(/^<\/?|\/?>$/g, "").split(/\s/)[0];
    const empty = tag.endsWith("/>");
    switch (name) {
      case "true":
      case "false":
        return name === "true";
      case "string":
        return empty ? "" : textUntil("</string>");
      case "integer": {
        const s = textUntil("</integer>").trim();
        if (!/^[-+]?(0x[0-9a-fA-F]+|\d+)$/.test(s)) throw new PlistError(`Not an integer: ${s}`);
        const n = BigInt(s.replace(/^\+/, ""));
        return n >= BigInt(Number.MIN_SAFE_INTEGER) && n <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(n) : n;
      }
      case "real": {
        const n = Number(textUntil("</real>").trim());
        if (Number.isNaN(n)) throw new PlistError("Not a real number.");
        return new PlistReal(n);
      }
      case "date": {
        const d = new Date(textUntil("</date>").trim());
        if (Number.isNaN(d.getTime())) throw new PlistError("Not a date.");
        return d;
      }
      case "data":
        return empty ? new Uint8Array() : Uint8Array.from(Buffer.from(textUntil("</data>").replace(/\s+/g, ""), "base64"));
      case "array": {
        const items: PlistValue[] = [];
        if (empty) return items;
        for (let t = next(); t !== "</array>"; t = next()) items.push(value(t));
        return items;
      }
      case "dict": {
        const dict = new Map<string, PlistValue>();
        if (empty) return dict;
        for (let t = next(); t !== "</dict>"; t = next()) {
          if (t !== "<key>") throw new PlistError(`Expected <key>, found ${t}.`);
          const key = textUntil("</key>");
          dict.set(key, value(next()));
        }
        // A keyed archive's reference reads back as one.
        const uid = dict.get("CF$UID");
        return dict.size === 1 && typeof uid === "number" ? new PlistUid(uid) : dict;
      }
      default:
        throw new PlistError(`Unknown element ${tag}.`);
    }
  };
  const open = next();
  if (!open.startsWith("<plist")) throw new PlistError("Not an XML property list.");
  const root = value(next());
  if (next() !== "</plist>") throw new PlistError("Expected </plist>.");
  return root;
}

/** A provisioning profile is a signed (CMS) envelope around an XML property list: its text. */
export function profileXml(bytes: Uint8Array): string | undefined {
  const text = Buffer.from(bytes).toString("latin1");
  const start = text.indexOf("<?xml");
  const end = text.indexOf("</plist>", start);
  return start >= 0 && end > start ? Buffer.from(text.slice(start, end + 8), "latin1").toString("utf8") + "\n" : undefined;
}
