// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The "Virtual iPhone" editor tab: a device frame with touch and keyboard
// capture and Xcode-Simulator-style hardware buttons (Home, Lock, Volume,
// Rotate, Screenshot). Every button goes through a host command, so the same
// actions work from the keyboard shortcuts and the editor title bar. The
// display stream and input delivery come from the engine's virtual device
// provider; until it exists the host says so in the status line.

import { vscodeApi } from "../shared/vscode";
import { addStyles, announcer, h } from "../shared/dom";
import { DeviceFrame, type FrameDevice } from "../shared/deviceFrame";
import type { Orientation } from "../shared/geometry";
import baseCss from "../shared/base.css?inline";
import deviceCss from "../shared/device.css?inline";
import pageCss from "./device.css?inline";

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

const frame = new DeviceFrame(stage, {
  interactive: true,
  onInput: (input) => post({ kind: "input", input }),
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
