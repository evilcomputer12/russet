// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The sidebar iPhone view: a compact device frame for the run destination. The
// interactive screen (touch, keyboard, hardware buttons) lives in the "Virtual
// iPhone" editor tab, which shares this frame (webview/shared/deviceFrame.ts);
// the decode pipeline arrives with the mirror stream (EXT-09). The message
// contract stays backend-neutral (device vs virtual).

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

window.addEventListener("message", (event: MessageEvent) => {
  const msg = event.data as {
    kind?: string;
    status?: string;
    target?: { name?: string; kind?: string; detail?: string; screen?: [number, number] };
  };
  if (msg.kind !== "state") {
    return;
  }
  const t = msg.target;
  if (!t?.name) {
    frame.setDevice({ name: "iPhone", detail: "", screen: [1206, 2622] });
    frame.setStatus({ title: "No run destination", detail: "Connect an iPhone or create a virtual iPhone." });
    return;
  }
  frame.setDevice({ name: t.name, detail: t.detail ?? "", screen: t.screen ?? [1206, 2622] });
  frame.setStatus({
    title: t.name,
    detail:
      t.kind === "virtual"
        ? "Open the Virtual iPhone tab to interact with it."
        : "Screen mirroring for USB iPhones arrives with the mirror stream.",
  });
});

vscode.postMessage({ kind: "ready" });
