// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The embedded iPhone webview. Tier 0 draws a phone-shaped placeholder where the
// live mirror / virtual-iPhone emulator will render. The decode worker + HID input
// (WebCodecs over ws://127.0.0.1) arrive with WS-12; the message contract is already
// backend-neutral (device vs virtual).

import { vscodeApi } from "../shared/vscode";

const vscode = vscodeApi();
const root = document.getElementById("root") as HTMLElement;

root.innerHTML = `
  <div style="display:flex;align-items:center;justify-content:center;height:100vh;padding:12px">
    <div id="frame" style="width:min(260px,70vw);aspect-ratio:9/19.5;border:10px solid var(--vscode-panel-border);border-radius:28px;background:var(--vscode-editorWidget-background);display:flex;flex-direction:column;align-items:center;justify-content:center;gap:10px;text-align:center;padding:16px">
      <div style="font-size:28px">📱</div>
      <div id="title" style="font-weight:600">Virtual iPhone</div>
      <div id="status" style="opacity:0.75;font-size:12px">The embedded iPhone screen will render here.</div>
    </div>
  </div>
`;

const titleEl = document.getElementById("title") as HTMLElement;
const statusEl = document.getElementById("status") as HTMLElement;

window.addEventListener("message", (event: MessageEvent) => {
  const msg = event.data as {
    kind?: string;
    status?: string;
    target?: { name?: string; kind?: string };
  };
  if (msg.kind !== "state") {
    return;
  }
  if (msg.target?.name) {
    titleEl.textContent = msg.target.name;
    statusEl.textContent =
      msg.status === "placeholder"
        ? "Screen mirroring arrives in a later update. Press Mirror Screen to connect."
        : `Connected (${msg.target.kind ?? "device"}).`;
  } else {
    titleEl.textContent = "Virtual iPhone";
    statusEl.textContent = "Connect an iPhone to see its screen here.";
  }
});

vscode.postMessage({ kind: "ready" });
