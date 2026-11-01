// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The "Virtual iPhone" editor tab: a device frame with touch and keyboard
// capture and Xcode-Simulator-style hardware buttons (Home, Lock, Volume,
// Rotate, Screenshot). Every button goes through a host command, so the same
// actions work from the keyboard shortcuts and the editor title bar. While the
// virtual iPhone runs, the host sends a "stream" message: the page shows its
// screen from QEMU's VNC server and sends touches, keys and Home straight back
// (rfb.ts); otherwise input goes to the host, which says what it did with it.

import { vscodeApi } from "../shared/vscode";
import { addStyles, announcer, h } from "../shared/dom";
import { DeviceFrame, type FrameDevice } from "../shared/deviceFrame";
import type { Orientation } from "../shared/geometry";
import baseCss from "../shared/base.css?inline";
import deviceCss from "../shared/device.css?inline";
import pageCss from "./device.css?inline";
import { BUTTON_MIDDLE, BUTTON_PRIMARY, keysym, RfbClient } from "./rfb";

interface PanelState {
  device: FrameDevice & { kind: "virtual" | "device" };
  orientation: Orientation;
  status?: { title: string; detail?: string; action?: { id: string; label: string }; busy?: boolean };
  note?: string;
}

const vscode = vscodeApi();
addStyles(baseCss, deviceCss, pageCss);
const announce = announcer();
const post = (msg: unknown): void => vscode.postMessage(msg);

const titleEl = h("h1", { id: "device-title" }, "Virtual iPhone");
const detailEl = h("p", { class: "muted", id: "device-detail" });
const noteEl = h("p", { class: "note muted", role: "status", "aria-live": "polite" });

const BUTTONS: Array<{ id: string; label: string; icon: string; keys: string }> = [
  { id: "home", label: "Home", icon: "⌂", keys: "Ctrl+Shift+H" },
  { id: "lock", label: "Lock", icon: "⏻", keys: "Ctrl+Shift+L" },
  { id: "volumeUp", label: "Volume Up", icon: "+", keys: "Ctrl+Shift+Up" },
  { id: "volumeDown", label: "Volume Down", icon: "−", keys: "Ctrl+Shift+Down" },
  { id: "rotateLeft", label: "Rotate Left", icon: "⟲", keys: "Ctrl+Shift+Left" },
  { id: "rotateRight", label: "Rotate Right", icon: "⟳", keys: "Ctrl+Shift+Right" },
  { id: "screenshot", label: "Screenshot", icon: "⎙", keys: "Ctrl+Shift+S" },
];

const toolbar = h("div", { class: "hw", role: "group", "aria-label": "Hardware buttons" });
for (const b of BUTTONS) {
  toolbar.append(
    h(
      "button",
      {
        class: "secondary hw-button",
        title: `${b.label} (${b.keys})`,
        "aria-keyshortcuts": b.keys.replace(/\+/g, "+"),
        onclick: () => post({ kind: "button", name: b.id }),
      },
      h("span", { class: "hw-icon", "aria-hidden": "true" }, b.icon),
      h("span", { class: "hw-label" }, b.label),
    ),
  );
}

const stage = h("div", { class: "stage" });
document.getElementById("root")?.append(
  h(
    "main",
    { class: "device-page", "aria-labelledby": "device-title" },
    h("header", { class: "device-head" }, h("div", {}, titleEl, detailEl), toolbar),
    stage,
    h("footer", { class: "device-foot" }, noteEl),
  ),
);

/** The running virtual iPhone's screen stream, while there is one. */
let stream: { url: string; rfb: RfbClient; shown: boolean } | undefined;
let retry: ReturnType<typeof setTimeout> | undefined;

function startStream(url: string): void {
  if (stream?.url === url) {
    return;
  }
  stopStream();
  frame.setStatus({ title: "Connecting to the virtual iPhone's screen…", busy: true });
  const s: NonNullable<typeof stream> = {
    url,
    shown: false,
    rfb: new RfbClient(url, {
      frame: (canvas, width, height) => {
        frame.drawFrame(canvas, width, height);
        if (!s.shown) {
          s.shown = true;
          frame.setStatus(undefined);
        }
      },
      closed: (reason) => {
        if (stream !== s) {
          return;
        }
        stream = undefined;
        frame.setStatus({ title: "The virtual iPhone's screen is not reachable", detail: `${reason} Retrying…`, busy: true });
        // The VNC server comes up with QEMU; try again while the host still says it runs.
        retry = setTimeout(() => startStream(url), 3000);
      },
    }),
  };
  stream = s;
}

function stopStream(): void {
  if (retry) {
    clearTimeout(retry);
    retry = undefined;
  }
  const s = stream;
  stream = undefined;
  s?.rfb.close();
}

function pressHome(): void {
  stream?.rfb.pointer(0.5, 0.5, BUTTON_MIDDLE);
  stream?.rfb.pointer(0.5, 0.5, 0);
}

/** Where the touch in progress went down (0…1 of the screen's height). */
let touchStartY: number | undefined;
/**
 * A swipe up from the bottom tenth, by at least an eighth of the screen: the home gesture, which
 * the guest does not take from this touchscreen. It unlocks the phone (the host asks vphoned) and
 * presses Home (QEMU's shortcut).
 */
function homeGesture(endY: number): boolean {
  const start = touchStartY;
  touchStartY = undefined;
  return start !== undefined && start >= 0.9 && start - endY >= 0.12;
}

const frame = new DeviceFrame(stage, {
  interactive: true,
  onInput: (input) => {
    const rfb = stream?.rfb;
    if (!rfb) {
      post({ kind: "input", input });
    } else if (input.type === "touch") {
      if (input.phase === "down") {
        touchStartY = input.y;
      }
      rfb.pointer(input.x, input.y, input.phase === "up" ? 0 : BUTTON_PRIMARY);
      if (input.phase === "up" && homeGesture(input.y)) {
        post({ kind: "unlock" });
        pressHome();
      }
    } else {
      const sym = keysym(input.key);
      if (sym !== undefined) {
        rfb.key(sym, true);
        rfb.key(sym, false);
      }
    }
  },
  onSideButton: (name) => post({ kind: "button", name }),
});

let lastNote = "";

function apply(state: PanelState): void {
  titleEl.textContent = state.device.name;
  detailEl.textContent = state.device.detail;
  frame.setDevice(state.device);
  frame.setOrientation(state.orientation);
  frame.setStatus(
    state.status
      ? {
          title: state.status.title,
          ...(state.status.detail ? { detail: state.status.detail } : {}),
          ...(state.status.busy ? { busy: true } : {}),
          ...(state.status.action
            ? { action: { label: state.status.action.label, run: () => post({ kind: "action", id: state.status?.action?.id }) } }
            : {}),
        }
      : undefined,
  );
  const note = state.note ?? "";
  if (note !== lastNote) {
    lastNote = note;
    noteEl.textContent = note;
  }
}

window.addEventListener("message", (event: MessageEvent) => {
  const msg = event.data as { kind?: string; state?: PanelState; text?: string };
  switch (msg.kind) {
    case "state":
      if (msg.state) {
        apply(msg.state);
      }
      break;
    case "stream":
      if (typeof (msg as { url?: unknown }).url === "string") {
        startStream((msg as { url: string }).url);
      }
      break;
    case "stopStream":
      stopStream();
      break;
    case "home":
      pressHome();
      break;
    case "captureScreenshot":
      post({ kind: "screenshot", dataUrl: frame.snapshot() });
      break;
    case "announce":
      if (msg.text) {
        announce(msg.text);
      }
      break;
    default:
      break;
  }
});

post({ kind: "ready" });
