// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The Device Console webview script. A ring buffer plus incremental DOM appends to
// hold the 2,000 lines/s budget (ide.md §9). A test-only perf hook (perfPing →
// perfPong) lets the integration suite measure input-echo latency; never self-reported.

import { vscodeApi } from "../shared/vscode";

interface LogEntry {
  ts: string;
  source: string;
  level: string;
  message: string;
  pid?: number;
}

const vscode = vscodeApi();

let maxLines = 50000;
let filterPid: number | undefined;
const buffer: string[] = [];

const root = document.getElementById("root") as HTMLElement;
root.innerHTML = `
  <div id="toolbar" style="display:flex;gap:6px;padding:4px 6px;position:sticky;top:0;background:var(--vscode-editor-background);border-bottom:1px solid var(--vscode-panel-border)">
    <button id="clear">Clear</button>
    <button id="export">Export…</button>
    <span id="count" style="margin-left:auto;opacity:0.7"></span>
  </div>
  <div id="log" role="log" aria-live="polite" style="font-family:var(--vscode-editor-font-family,monospace);white-space:pre-wrap;padding:4px 6px;overflow:auto;height:calc(100vh - 34px)"></div>
`;

const logEl = document.getElementById("log") as HTMLElement;
const countEl = document.getElementById("count") as HTMLElement;

function levelColor(level: string): string {
  switch (level) {
    case "error":
    case "fault":
      return "var(--vscode-errorForeground)";
    case "warn":
      return "var(--vscode-editorWarning-foreground)";
    default:
      return "var(--vscode-foreground)";
  }
}

function appendEntries(entries: LogEntry[]): void {
  const atBottom = logEl.scrollTop + logEl.clientHeight >= logEl.scrollHeight - 4;
  const frag = document.createDocumentFragment();
  for (const e of entries) {
    if (filterPid !== undefined && e.pid !== undefined && e.pid !== filterPid) {
      continue;
    }
    const line = document.createElement("div");
    line.textContent = e.message; // untrusted text; never innerHTML
    line.style.color = levelColor(e.level);
    frag.appendChild(line);
    buffer.push(e.message);
  }
  logEl.appendChild(frag);
  // Trim the ring buffer and the DOM.
  while (buffer.length > maxLines) {
    buffer.shift();
    if (logEl.firstChild) {
      logEl.removeChild(logEl.firstChild);
    }
  }
  countEl.textContent = `${buffer.length} lines`;
  if (atBottom) {
    logEl.scrollTop = logEl.scrollHeight;
  }
}

window.addEventListener("message", (event: MessageEvent) => {
  const msg = event.data as { kind?: string; [k: string]: unknown };
  switch (msg.kind) {
    case "init":
      maxLines = (msg.maxLines as number) ?? maxLines;
      break;
    case "append":
      appendEntries((msg.entries as LogEntry[]) ?? []);
      break;
    case "filter":
      filterPid = msg.pid as number | undefined;
      break;
    case "clear":
      buffer.length = 0;
      logEl.textContent = "";
      countEl.textContent = "0 lines";
      break;
    case "perfPing":
      // Echo straight back so the host can measure round-trip latency.
      vscode.postMessage({ kind: "perfPong", id: msg.id, t: msg.t });
      break;
    default:
      break;
  }
});

document.getElementById("clear")?.addEventListener("click", () => {
  buffer.length = 0;
  logEl.textContent = "";
  countEl.textContent = "0 lines";
});
document.getElementById("export")?.addEventListener("click", () => {
  vscode.postMessage({ kind: "export", text: buffer.join("\n") });
});

vscode.postMessage({ kind: "ready" });
