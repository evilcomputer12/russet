// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The sidebar iPhone view: a device frame for the run destination. A physical
// iPhone streams: the extension starts `screen.mirror.start` and hands over the
// WebSocket ("stream" messages); this page decodes the HEVC access units with
// WebCodecs (ARCHITECTURE §10.1) and sends touch, keys and the side buttons
// back. When it cannot decode HEVC (an editor without the GPU's decoder), the
// stream stays open for touch, keys and buttons ("inputOnly") while the
// extension polls JPEG screenshots ("frame" messages), one right after each
// touch; a stream without input ends ("streamEnded" with `unsupported`). The
// label under the phone says which. The virtual iPhone's interactive screen is
// the "Virtual iPhone" editor tab, which shares this frame
// (webview/shared/deviceFrame.ts).

import { vscodeApi } from "../shared/vscode";
import { addStyles, h } from "../shared/dom";
import { DeviceFrame, type FrameInput, type SideButton } from "../shared/deviceFrame";
import baseCss from "../shared/base.css?inline";
import deviceCss from "../shared/device.css?inline";

const vscode = vscodeApi();
addStyles(
  baseCss,
  deviceCss,
  `#root{height:100vh;display:flex;flex-direction:column}.stage{flex:1;min-height:0;display:flex;align-items:center;justify-content:center;padding:8px}.foot{padding:4px 8px 10px;display:flex;gap:6px;justify-content:center}`,
);

/** HID keyboard usages of the named keys the device understands (the printable ones go as text). */
const KEY_USAGES: Record<string, number> = {
  Enter: 0x28,
  Backspace: 0x2a,
  Delete: 0x4c,
  ArrowRight: 0x4f,
  ArrowLeft: 0x50,
  ArrowDown: 0x51,
  ArrowUp: 0x52,
  Home: 0x4a,
  End: 0x4d,
  PageUp: 0x4b,
  PageDown: 0x4e,
};

const SIDE_BUTTONS: Record<SideButton, string> = { volumeUp: "volume-up", volumeDown: "volume-down", lock: "lock" };

const stage = h("div", { class: "stage" });
/** How the screen arrives: live video, or screenshots and how many a second. */
const mode = h("span", { class: "muted", style: "align-self:center;font-size:11px" });
const open = h("button", { class: "secondary", onclick: () => vscode.postMessage({ kind: "openEditor" }) }, "Open in Editor");
const home = h("button", { class: "secondary", title: "Go to the Home Screen", onclick: () => send({ t: "button", button: "home" }) }, "Home");
home.hidden = true;
// A virtual iPhone is shown and driven in its editor tab: no second phone frame here.
const virtualNote = h("p", { class: "muted", style: "padding:12px;text-align:center" }, "The virtual iPhone runs in its editor tab.");
virtualNote.hidden = true;
document.getElementById("root")?.append(virtualNote, stage, h("div", { class: "foot" }, home, open, mode));
const frame = new DeviceFrame(stage, { interactive: true, onInput: input, onSideButton: (b) => send({ t: "button", button: SIDE_BUTTONS[b] }) });
frame.setStatus({ title: "iPhone", detail: "Connect an iPhone or create a virtual iPhone to see its screen here." });

// The destination shown, and the one whose screen is on the canvas.
let current: { udid?: string; name: string; detail: string; screen: [number, number] } | undefined;
let shown: string | undefined;

/**
 * The live stream: its socket and decoder, while the extension has one running for `udid`. Without `pictures`
 * (this editor cannot decode the video) it carries only touch, keys and buttons, and the screen comes from
 * screenshots.
 */
let stream: { udid: string; ws: WebSocket; decoder?: VideoDecoder; input: boolean; pictures: boolean; ping?: number } | undefined;
/** Screenshots shown in the last two seconds, for the frame rate under the phone. */
const shots: number[] = [];
/** The latest decoded picture, drawn on the next animation frame. */
let pending: VideoFrame | undefined;

function send(message: object): void {
  if (stream?.input && stream.ws.readyState === WebSocket.OPEN) {
    stream.ws.send(JSON.stringify(message));
  }
}

/** On screenshots, the screen right after a touch or a key, not at the next tick. */
function touched(): void {
  if (stream && !stream.pictures) {
    vscode.postMessage({ kind: "touched" });
  }
}

function input(e: FrameInput): void {
  if (e.type === "touch") {
    const scale = (v: number): number => Math.max(0, Math.min(65535, Math.round(v * 65535)));
    send({ t: "touch", phase: e.phase, x: scale(e.x), y: scale(e.y) });
    if (e.phase === "up") touched();
    return;
  }
  touched();
  if (e.key.length === 1) {
    send({ t: "text", text: e.key });
  } else if (KEY_USAGES[e.key] !== undefined) {
    send({ t: "key", usage: KEY_USAGES[e.key], down: true });
    send({ t: "key", usage: KEY_USAGES[e.key], down: false });
  }
}

/** Closes the current stream's socket and decoder, without telling the extension. */
function teardown(): NonNullable<typeof stream> | undefined {
  const s = stream;
  if (!s) {
    return undefined;
  }
  stream = undefined;
  home.hidden = true;
  mode.textContent = "";
  window.clearInterval(s.ping);
  try {
    s.ws.close();
  } catch {
    // already closed
  }
  if (s.decoder && s.decoder.state !== "closed") {
    s.decoder.close();
  }
  return s;
}

/** The mode label: `text`, or for screenshots their rate. */
function setMode(text: string | undefined): void {
  if (text !== undefined) {
    mode.textContent = text;
    return;
  }
  const now = performance.now();
  while (shots.length > 0 && now - shots[0] > 2000) shots.shift();
  const control = stream?.input && !stream.pictures ? ", tap to control" : "";
  mode.textContent = `Screenshots · ${Math.round(shots.length / 2)}/s${control}`;
}

/** Ends the stream and tells the extension, which polls screenshots meanwhile. */
function endStream(reason: string, unsupported = false): void {
  const s = teardown();
  if (!s) {
    return;
  }
  if (current?.udid === s.udid) {
    shown = undefined;
    frame.setStatus({ title: current.name, detail: reason, busy: true });
  }
  vscode.postMessage({ kind: "streamEnded", udid: s.udid, reason, unsupported });
}

function draw(): void {
  const picture = pending;
  pending = undefined;
  if (!picture) {
    return;
  }
  const target = current;
  if (target && stream && target.udid === stream.udid) {
    const [width, height] = [picture.displayWidth, picture.displayHeight];
    if (target.screen[0] !== width || target.screen[1] !== height) {
      target.screen = [width, height];
      frame.setDevice({ name: target.name, detail: target.detail, screen: target.screen });
    }
    frame.drawFrame(picture, width, height);
    if (shown !== target.udid) {
      frame.setStatus(undefined);
      shown = target.udid;
    }
  }
  picture.close();
}

/**
 * This editor cannot decode the stream: the screen comes from screenshots, and the stream stays open for touch,
 * keys and buttons when the device takes them; without input there is no use for it.
 */
function noPictures(s: NonNullable<typeof stream>, codec: string, reason: string): void {
  if (!s.input) {
    return endStream(`${reason} Showing screenshots.`, true);
  }
  s.pictures = false;
  home.hidden = false;
  vscode.postMessage({ kind: "inputOnly", udid: s.udid, codec, reason });
  setMode(undefined);
}

async function configure(s: NonNullable<typeof stream>, config: { codec: string; codedWidth?: number; codedHeight?: number }): Promise<void> {
  if (typeof VideoDecoder === "undefined") {
    return noPictures(s, config.codec, "This editor cannot decode video.");
  }
  const wanted: VideoDecoderConfig = {
    codec: config.codec,
    ...(config.codedWidth && config.codedHeight ? { codedWidth: config.codedWidth, codedHeight: config.codedHeight } : {}),
    optimizeForLatency: true,
  };
  const support = await VideoDecoder.isConfigSupported(wanted).catch(() => undefined);
  if (stream !== s) {
    return;
  }
  if (!support?.supported) {
    return noPictures(s, config.codec, "This editor cannot decode the iPhone's HEVC video (it needs the GPU's HEVC decoder).");
  }
  setMode("Live video");
  const decoder = new VideoDecoder({
    output: (picture) => {
      if (pending) {
        pending.close();
      } else {
        requestAnimationFrame(draw);
      }
      pending = picture;
    },
    error: (e) => endStream(`The video decoder stopped: ${e.message}`),
  });
  decoder.configure(wanted);
  s.decoder = decoder;
}

function startStream(msg: { udid: string; url: string; token: string; width?: number; height?: number }): void {
  teardown();
  const ws = new WebSocket(msg.url, "russet.mirror.v1");
  ws.binaryType = "arraybuffer";
  const s: NonNullable<typeof stream> = { udid: msg.udid, ws, input: false, pictures: true };
  stream = s;
  // Wait for a keyframe after every (re)configure.
  let needKey = true;
  ws.onopen = () => ws.send(JSON.stringify({ t: "auth", token: msg.token }));
  ws.onmessage = (event: MessageEvent) => {
    if (stream !== s) {
      return;
    }
    if (typeof event.data === "string") {
      const m = JSON.parse(event.data) as { t?: string; codec?: string; codedWidth?: number; codedHeight?: number; input?: boolean; message?: string };
      if (m.t === "config" && m.codec) {
        s.input = m.input === true;
        home.hidden = !s.input;
        needKey = true;
        void configure(s, { codec: m.codec, codedWidth: m.codedWidth, codedHeight: m.codedHeight });
      } else if (m.t === "end") {
        endStream(m.message ?? "The screen stream ended.");
      }
      return;
    }
    const data = event.data as ArrayBuffer;
    const decoder = s.decoder;
    if (data.byteLength <= 16 || !decoder || decoder.state !== "configured") {
      return;
    }
    const header = new DataView(data, 0, 16);
    const key = (header.getUint8(2) & 1) === 1;
    if (needKey && !key) {
      return;
    }
    needKey = false;
    const timestamp = Number(header.getBigUint64(8, true));
    try {
      decoder.decode(new EncodedVideoChunk({ type: key ? "key" : "delta", timestamp, data: new Uint8Array(data, 16) }));
    } catch (e) {
      endStream(`The video decoder refused a frame: ${(e as Error).message}`);
    }
  };
  ws.onclose = () => {
    if (stream === s) {
      endStream("The screen stream closed.");
    }
  };
  ws.onerror = () => {
    if (stream === s) {
      endStream("Cannot reach the screen stream.");
    }
  };
  s.ping = window.setInterval(() => {
    if (ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ t: "ping" }));
    }
  }, 10_000);
}

window.addEventListener("message", (event: MessageEvent) => {
  const msg = event.data as {
    kind?: string;
    status?: string;
    udid?: string;
    url?: string;
    token?: string;
    image?: string;
    format?: string;
    width?: number;
    height?: number;
    title?: string;
    detail?: string;
    target?: { udid?: string; name?: string; kind?: string; detail?: string; screen?: [number, number] };
  };
  if (msg.kind === "stream" && msg.udid && msg.url && msg.token) {
    startStream({ udid: msg.udid, url: msg.url, token: msg.token, width: msg.width, height: msg.height });
    return;
  }
  if (msg.kind === "stopStream") {
    teardown();
    return;
  }
  if (msg.kind === "frame") {
    const target = current;
    // Screenshots stand in for a stream only while it has no pictures.
    const live = stream?.udid === target?.udid && stream?.pictures;
    if (!target || msg.udid !== target.udid || !msg.image || !msg.width || !msg.height || live) {
      return;
    }
    const [width, height] = [msg.width, msg.height];
    const image = new Image();
    image.onload = () => {
      if (current !== target) {
        return;
      }
      if (target.screen[0] !== width || target.screen[1] !== height) {
        target.screen = [width, height];
        frame.setDevice({ name: target.name, detail: target.detail, screen: target.screen });
      }
      frame.drawFrame(image, width, height);
      frame.setStatus(undefined);
      shown = target.udid;
      shots.push(performance.now());
      setMode(undefined);
    };
    image.src = `data:image/${msg.format === "jpeg" ? "jpeg" : "png"};base64,${msg.image}`;
    return;
  }
  if (msg.kind === "mirrorStatus") {
    if (current && msg.udid === current.udid) {
      frame.setStatus({ title: msg.title ?? "No screen", detail: msg.detail });
      shown = undefined;
    }
    return;
  }
  if (msg.kind !== "state") {
    return;
  }
  const t = msg.target;
  if (!t?.name) {
    stage.hidden = false;
    virtualNote.hidden = true;
    current = undefined;
    shown = undefined;
    frame.setDevice({ name: "iPhone", detail: "", screen: [1206, 2622] });
    frame.setStatus({ title: "No run destination", detail: "Connect an iPhone or create a virtual iPhone." });
    return;
  }
  const same = current?.udid === t.udid;
  current = { udid: t.udid, name: t.name, detail: t.detail ?? "", screen: same && current ? current.screen : (t.screen ?? [1206, 2622]) };
  frame.setDevice({ name: current.name, detail: current.detail, screen: current.screen });
  stage.hidden = t.kind === "virtual";
  virtualNote.hidden = t.kind !== "virtual";
  if (t.kind === "virtual") {
    frame.setStatus({ title: t.name, detail: "Open the Virtual iPhone tab to interact with it." });
  } else if (msg.status !== "mirroring") {
    frame.setStatus({ title: t.name, detail: "Screen mirroring needs the Russet backend." });
  } else if (shown !== t.udid) {
    frame.setStatus({ title: t.name, detail: "Connecting to the screen…", busy: true });
  }
});

vscode.postMessage({ kind: "ready" });
