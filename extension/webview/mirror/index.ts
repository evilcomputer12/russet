// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The sidebar iPhone view: a compact device frame for the run destination. The
// interactive screen (touch, keyboard, hardware buttons) lives in the "Virtual
// iPhone" editor tab, which shares this frame (webview/shared/deviceFrame.ts).
// A physical iPhone shows the PNG screenshots the extension polls ("frame"
// messages); the decode pipeline arrives with the mirror stream (EXT-09). The
// message contract stays backend-neutral (device vs virtual).

import { vscodeApi } from "../shared/vscode";
import { addStyles, h } from "../shared/dom";
import { DeviceFrame } from "../shared/deviceFrame";
import baseCss from "../shared/base.css?inline";
import deviceCss from "../shared/device.css?inline";

const vscode = vscodeApi();
addStyles(
  baseCss,
  deviceCss,
  `#root{height:100vh;display:flex;flex-direction:column}.stage{flex:1;min-height:0;display:flex;align-items:center;justify-content:center;padding:8px}.foot{padding:4px 8px 10px;text-align:center}`,
);

const stage = h("div", { class: "stage" });
const open = h("button", { class: "secondary", onclick: () => vscode.postMessage({ kind: "openEditor" }) }, "Open in Editor");
document.getElementById("root")?.append(stage, h("div", { class: "foot" }, open));
const frame = new DeviceFrame(stage, { interactive: false });
frame.setStatus({ title: "iPhone", detail: "Connect an iPhone or create a virtual iPhone to see its screen here." });

// The destination shown, and the one whose screen is on the canvas.
let current: { udid?: string; name: string; detail: string; screen: [number, number] } | undefined;
let shown: string | undefined;

window.addEventListener("message", (event: MessageEvent) => {
  const msg = event.data as {
    kind?: string;
    status?: string;
    udid?: string;
    png?: string;
    width?: number;
    height?: number;
    title?: string;
    detail?: string;
    target?: { udid?: string; name?: string; kind?: string; detail?: string; screen?: [number, number] };
  };
  if (msg.kind === "frame") {
    const target = current;
    if (!target || msg.udid !== target.udid || !msg.png || !msg.width || !msg.height) {
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
    };
    image.src = `data:image/png;base64,${msg.png}`;
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
    current = undefined;
    shown = undefined;
    frame.setDevice({ name: "iPhone", detail: "", screen: [1206, 2622] });
    frame.setStatus({ title: "No run destination", detail: "Connect an iPhone or create a virtual iPhone." });
    return;
  }
  const same = current?.udid === t.udid;
  current = { udid: t.udid, name: t.name, detail: t.detail ?? "", screen: same && current ? current.screen : (t.screen ?? [1206, 2622]) };
  frame.setDevice({ name: current.name, detail: current.detail, screen: current.screen });
  if (t.kind === "virtual") {
    frame.setStatus({ title: t.name, detail: "Open the Virtual iPhone tab to interact with it." });
  } else if (msg.status !== "mirroring") {
    frame.setStatus({ title: t.name, detail: "Screen mirroring needs the Russet backend." });
  } else if (shown !== t.udid) {
    frame.setStatus({ title: t.name, detail: "Connecting to the screen…", busy: true });
  }
});

vscode.postMessage({ kind: "ready" });
