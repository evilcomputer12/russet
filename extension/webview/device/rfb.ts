// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// A small RFB (VNC) client over a WebSocket, for the virtual iPhone's screen: QEMU serves the
// guest's display and takes its pointer and keys (`-vnc …,websocket=…`), and its USB
// touchscreen turns the pointer into touches. Raw encoding only, on the loopback; the
// server's pixels are asked for as 32-bit little-endian R, G, B, X, so a rectangle copies
// straight into an ImageData. RFB 3.8, security type None (RFC 6143).

export interface RfbView {
  /** A frame is complete: draw `canvas` (`width` × `height`). */
  frame(canvas: HTMLCanvasElement, width: number, height: number): void;
  /** The connection ended (`reason` for the user). */
  closed(reason: string): void;
}

/** RFB pointer buttons (`buttonMask` bits). */
export const BUTTON_PRIMARY = 1;
export const BUTTON_MIDDLE = 2;

const ENCODING_RAW = 0;
const ENCODING_DESKTOP_SIZE = -223;

/** X11 keysyms of the named keys a phone keyboard has; printable characters are their code. */
const KEYSYMS: Record<string, number> = {
  Enter: 0xff0d, Backspace: 0xff08, Tab: 0xff09, Escape: 0xff1b, Delete: 0xffff,
  ArrowLeft: 0xff51, ArrowUp: 0xff52, ArrowRight: 0xff53, ArrowDown: 0xff54,
  Home: 0xff50, End: 0xff57, PageUp: 0xff55, PageDown: 0xff56, Shift: 0xffe1,
};

export function keysym(key: string): number | undefined {
  if (KEYSYMS[key] !== undefined) {
    return KEYSYMS[key];
  }
  const cp = key.length <= 2 ? key.codePointAt(0) : undefined;
  if (cp === undefined || key.length !== String.fromCodePoint(cp).length) {
    return undefined;
  }
  // Latin-1 keysyms are their code points; others are 0x01000000 + the code point.
  return cp < 0x100 ? cp : 0x01000000 + cp;
}

export class RfbClient {
  private readonly ws: WebSocket;
  private buf = new Uint8Array(0);
  private state: "version" | "security" | "result" | "init" | "normal" = "version";
  private width = 0;
  private height = 0;
  private image?: ImageData;
  private readonly canvas = document.createElement("canvas");
  private closedOnce = false;

  constructor(url: string, private readonly view: RfbView) {
    this.ws = new WebSocket(url, ["binary"]);
    this.ws.binaryType = "arraybuffer";
    this.ws.onmessage = (e: MessageEvent) => {
      this.append(new Uint8Array(e.data as ArrayBuffer));
      try {
        this.parse();
      } catch (err) {
        this.close(`The screen stream sent something unexpected: ${(err as Error).message}`);
      }
    };
    this.ws.onclose = () => this.close("The virtual iPhone's screen stream closed.");
    this.ws.onerror = () => this.close("Cannot reach the virtual iPhone's screen.");
  }

  get size(): [number, number] {
    return [this.width, this.height];
  }

  close(reason = "Closed."): void {
    if (this.closedOnce) {
      return;
    }
    this.closedOnce = true;
    try {
      this.ws.close();
    } catch {
      // already closed
    }
    this.view.closed(reason);
  }

  /** The pointer at `x`, `y` (0…1 of the screen) with `buttons` held. */
  pointer(x: number, y: number, buttons: number): void {
    if (this.state !== "normal" || this.width === 0) {
      return;
    }
    const px = Math.max(0, Math.min(this.width - 1, Math.round(x * (this.width - 1))));
    const py = Math.max(0, Math.min(this.height - 1, Math.round(y * (this.height - 1))));
    const m = new DataView(new ArrayBuffer(6));
    m.setUint8(0, 5);
    m.setUint8(1, buttons);
    m.setUint16(2, px);
    m.setUint16(4, py);
    this.send(m.buffer);
  }

  key(sym: number, down: boolean): void {
    if (this.state !== "normal") {
      return;
    }
    const m = new DataView(new ArrayBuffer(8));
    m.setUint8(0, 4);
    m.setUint8(1, down ? 1 : 0);
    m.setUint32(4, sym);
    this.send(m.buffer);
  }

  private send(data: ArrayBuffer | Uint8Array): void {
    if (this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(data);
    }
  }

  private append(chunk: Uint8Array): void {
    const next = new Uint8Array(this.buf.length + chunk.length);
    next.set(this.buf);
    next.set(chunk, this.buf.length);
    this.buf = next;
  }

  private take(n: number): Uint8Array {
    const out = this.buf.subarray(0, n);
    this.buf = this.buf.subarray(n);
    return out;
  }

  private u16(at: number): number {
    return (this.buf[at] << 8) | this.buf[at + 1];
  }

  private u32(at: number): number {
    return ((this.buf[at] << 24) >>> 0) + ((this.buf[at + 1] << 16) | (this.buf[at + 2] << 8) | this.buf[at + 3]);
  }

  private parse(): void {
    for (;;) {
      switch (this.state) {
        case "version": {
          if (this.buf.length < 12) {
            return;
          }
          const v = new TextDecoder().decode(this.take(12));
          if (!v.startsWith("RFB ")) {
            throw new Error("not a VNC server");
          }
          this.send(new TextEncoder().encode("RFB 003.008\n"));
          this.state = "security";
          break;
        }
        case "security": {
          if (this.buf.length < 1 || this.buf.length < 1 + this.buf[0]) {
            return;
          }
          const n = this.buf[0];
          if (n === 0) {
            throw new Error("the server refused the connection");
          }
          const types = this.take(1 + n).subarray(1);
          if (!types.includes(1)) {
            throw new Error("the server wants a password");
          }
          this.send(new Uint8Array([1]));
          this.state = "result";
          break;
        }
        case "result": {
          if (this.buf.length < 4) {
            return;
          }
          if (this.u32(0) !== 0) {
            throw new Error("the server refused the connection");
          }
          this.take(4);
          this.send(new Uint8Array([1])); // ClientInit: shared
          this.state = "init";
          break;
        }
        case "init": {
          if (this.buf.length < 24 || this.buf.length < 24 + this.u32(20)) {
            return;
          }
          this.resize(this.u16(0), this.u16(2));
          this.take(24 + this.u32(20));
          this.setup();
          this.state = "normal";
          break;
        }
        case "normal":
          if (!this.message()) {
            return;
          }
          break;
      }
    }
  }

  private setup(): void {
    // SetPixelFormat: 32 bpp, depth 24, little-endian, true colour, R at bit 0, G 8, B 16.
    const pf = new DataView(new ArrayBuffer(20));
    pf.setUint8(0, 0);
    pf.setUint8(4, 32);
    pf.setUint8(5, 24);
    pf.setUint8(6, 0);
    pf.setUint8(7, 1);
    pf.setUint16(8, 255);
    pf.setUint16(10, 255);
    pf.setUint16(12, 255);
    pf.setUint8(14, 0);
    pf.setUint8(15, 8);
    pf.setUint8(16, 16);
    this.send(pf.buffer);
    const encodings = [ENCODING_RAW, ENCODING_DESKTOP_SIZE];
    const se = new DataView(new ArrayBuffer(4 + 4 * encodings.length));
    se.setUint8(0, 2);
    se.setUint16(2, encodings.length);
    encodings.forEach((e, i) => se.setInt32(4 + 4 * i, e));
    this.send(se.buffer);
    this.request(false);
  }

  private request(incremental: boolean): void {
    const m = new DataView(new ArrayBuffer(10));
    m.setUint8(0, 3);
    m.setUint8(1, incremental ? 1 : 0);
    m.setUint16(6, this.width);
    m.setUint16(8, this.height);
    this.send(m.buffer);
  }

  private resize(width: number, height: number): void {
    this.width = width;
    this.height = height;
    this.canvas.width = width;
    this.canvas.height = height;
    this.image = width > 0 && height > 0 ? new ImageData(width, height) : undefined;
  }

  /** Handles one whole server message if it has arrived; false while it has not. */
  private message(): boolean {
    if (this.buf.length < 1) {
      return false;
    }
    switch (this.buf[0]) {
      case 0:
        return this.update();
      case 2: // Bell
        this.take(1);
        return true;
      case 3: {
        // ServerCutText
        if (this.buf.length < 8 || this.buf.length < 8 + this.u32(4)) {
          return false;
        }
        this.take(8 + this.u32(4));
        return true;
      }
      default:
        throw new Error(`message type ${this.buf[0]}`);
    }
  }

  /** A FramebufferUpdate, taken only once every rectangle in it has arrived. */
  private update(): boolean {
    if (this.buf.length < 4) {
      return false;
    }
    const rects = this.u16(2);
    let at = 4;
    for (let i = 0; i < rects; i++) {
      if (this.buf.length < at + 12) {
        return false;
      }
      const w = this.u16(at + 4);
      const h = this.u16(at + 6);
      const encoding = (this.u32(at + 8) | 0);
      at += 12;
      if (encoding === ENCODING_RAW) {
        at += w * h * 4;
        if (this.buf.length < at) {
          return false;
        }
      } else if (encoding !== ENCODING_DESKTOP_SIZE) {
        throw new Error(`encoding ${encoding}`);
      }
    }
    const msg = this.take(at);
    const view = new DataView(msg.buffer, msg.byteOffset, msg.byteLength);
    let p = 4;
    for (let i = 0; i < rects; i++) {
      const x = view.getUint16(p);
      const y = view.getUint16(p + 2);
      const w = view.getUint16(p + 4);
      const h = view.getUint16(p + 6);
      const encoding = view.getInt32(p + 8);
      p += 12;
      if (encoding === ENCODING_DESKTOP_SIZE) {
        this.resize(w, h);
        continue;
      }
      const image = this.image;
      if (image) {
        for (let row = 0; row < h; row++) {
          const src = msg.subarray(p + row * w * 4, p + (row + 1) * w * 4);
          image.data.set(src, ((y + row) * this.width + x) * 4);
          for (let i4 = ((y + row) * this.width + x) * 4 + 3, end = i4 + w * 4; i4 < end; i4 += 4) {
            image.data[i4] = 255;
          }
        }
      }
      p += w * h * 4;
    }
    if (this.image) {
      this.canvas.getContext("2d")?.putImageData(this.image, 0, 0);
      this.view.frame(this.canvas, this.width, this.height);
    }
    this.request(true);
    return true;
  }
}
