// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// russet-ide: an MCP server (stdio, newline-delimited JSON-RPC) that lets an AI assistant look at
// the Russet IDE itself when something in it misbehaves: its logs, the setup and backend status,
// its settings and a screenshot of the window. It reads; it changes nothing. The editor's own
// Electron runs it with ELECTRON_RUN_AS_NODE=1, so it needs no Node install.
//
//   RUSSET_IDE_DATA      the editor's user data folder (logs, User/settings.json)
//   RUSSET_WINDOWS_HOST  Russet.Host.exe, for `--setup status`
//   RUSSET_IDE_CDP_PORT  the editor's --remote-debugging-port, when started with one (screenshots)
"use strict";
const fs = require("node:fs");
const path = require("node:path");
const { execFile } = require("node:child_process");

const DATA = process.env.RUSSET_IDE_DATA || path.join(process.env.LOCALAPPDATA || "", "Russet", "IDE-Development");
const HOST = process.env.RUSSET_WINDOWS_HOST || "";
const CDP = Number(process.env.RUSSET_IDE_CDP_PORT || 9229);

const LOGS = {
  russet: ["window1", "exthost", "russet.russet", "Russet.log"],
  exthost: ["window1", "exthost", "exthost.log"],
  renderer: ["window1", "renderer.log"],
  main: ["main.log"],
  shared: ["sharedprocess.log"],
};

function latestLogs() {
  const root = path.join(DATA, "logs");
  const dirs = fs.existsSync(root) ? fs.readdirSync(root).filter((d) => /^\d{8}T\d{6}$/.test(d)).sort() : [];
  if (!dirs.length) throw new Error(`No editor logs under ${root}.`);
  return path.join(root, dirs[dirs.length - 1]);
}

function readLog(source, lines, grep) {
  const parts = LOGS[source];
  if (!parts) throw new Error(`Unknown log ${source}; one of ${Object.keys(LOGS).join(", ")}.`);
  const file = path.join(latestLogs(), ...parts);
  if (!fs.existsSync(file)) return `(${source} has no log in this session)`;
  let rows = fs.readFileSync(file, "utf8").split(/\r?\n/);
  if (grep) {
    const re = new RegExp(grep, "i");
    rows = rows.filter((r) => re.test(r));
  }
  return rows.slice(-Math.min(Math.max(lines || 200, 1), 2000)).join("\n");
}

function run(file, args, timeoutMs) {
  return new Promise((resolve) => {
    execFile(file, args, { timeout: timeoutMs, windowsHide: true, maxBuffer: 4 << 20 }, (err, stdout, stderr) =>
      resolve({ ok: !err, stdout: String(stdout), stderr: String(stderr) }));
  });
}

async function status() {
  const out = { userData: DATA };
  try {
    const product = JSON.parse(fs.readFileSync(path.join(path.dirname(process.execPath), "resources", "app", "product.json"), "utf8"));
    out.editor = { name: product.nameShort, version: product.version };
  } catch { /* not under the editor */ }
  try {
    const log = readLog("russet", 2000, "\\[daemon\\] (state|connected)").split("\n");
    out.backend = log[log.length - 1] || "unknown";
  } catch (e) { out.backend = String(e.message); }
  if (HOST) {
    const setup = await run(HOST, ["--setup", "status"], 60000);
    try { out.setup = JSON.parse(setup.stdout).steps; } catch { out.setup = setup.stderr.slice(0, 500) || "unavailable"; }
    const updates = await run(HOST, ["--setup", "updates"], 60000);
    try { out.components = JSON.parse(updates.stdout).components; } catch { /* offline */ }
  }
  return JSON.stringify(out, null, 2);
}

async function screenshot() {
  const list = await (await fetch(`http://127.0.0.1:${CDP}/json/list`)).json();
  const page = list.find((t) => t.type === "page" && t.url.includes("workbench"));
  if (!page) throw new Error("The editor's window was not found.");
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = () => reject(new Error("No connection to the editor.")); });
  const shot = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("The editor did not answer.")), 15000);
    ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id === 1) { clearTimeout(timer); resolve(m.result?.data); } };
    ws.send(JSON.stringify({ id: 1, method: "Page.captureScreenshot", params: { format: "png" } }));
  });
  ws.close();
  return shot;
}

const TOOLS = [
  { name: "ide_status", description: "Russet IDE health: editor version, backend connection, setup steps and managed components (virtual iPhone engine).",
    inputSchema: { type: "object", properties: {} } },
  { name: "ide_logs", description: "Recent lines of a Russet IDE log from the current session: russet (the Russet extension), exthost, renderer, main or shared.",
    inputSchema: { type: "object", properties: { source: { type: "string", enum: Object.keys(LOGS) }, lines: { type: "integer", minimum: 1, maximum: 2000 }, grep: { type: "string", description: "Case-insensitive regular expression to filter lines" } }, required: ["source"] } },
  { name: "ide_settings", description: "The Russet IDE's user settings (settings.json).", inputSchema: { type: "object", properties: {} } },
  { name: "ide_screenshot", description: "A screenshot of the Russet IDE window (needs the editor started with --remote-debugging-port).",
    inputSchema: { type: "object", properties: {} } },
];

async function call(name, args) {
  switch (name) {
    case "ide_status": return [{ type: "text", text: await status() }];
    case "ide_logs": return [{ type: "text", text: readLog(args.source, args.lines, args.grep) }];
    case "ide_settings": return [{ type: "text", text: fs.readFileSync(path.join(DATA, "User", "settings.json"), "utf8") }];
    case "ide_screenshot": return [{ type: "image", data: await screenshot(), mimeType: "image/png" }];
    default: throw new Error(`Unknown tool ${name}.`);
  }
}

function send(message) {
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...message }) + "\n");
}

async function handle(m) {
  if (m.id === undefined) return; // notifications
  try {
    if (m.method === "initialize") {
      send({ id: m.id, result: { protocolVersion: m.params?.protocolVersion || "2025-06-18", capabilities: { tools: {} },
        serverInfo: { name: "russet-ide", title: "Russet IDE", version: "0.1.0" },
        instructions: "Read-only view of the Russet IDE for diagnosing its bugs: start with ide_status, then ide_logs." } });
    } else if (m.method === "tools/list") {
      send({ id: m.id, result: { tools: TOOLS } });
    } else if (m.method === "tools/call") {
      try {
        send({ id: m.id, result: { content: await call(m.params?.name, m.params?.arguments || {}) } });
      } catch (e) {
        send({ id: m.id, result: { content: [{ type: "text", text: String(e.message || e) }], isError: true } });
      }
    } else if (m.method === "ping") {
      send({ id: m.id, result: {} });
    } else {
      send({ id: m.id, error: { code: -32601, message: `Unknown method ${m.method}` } });
    }
  } catch (e) {
    send({ id: m.id, error: { code: -32603, message: String(e.message || e) } });
  }
}

let buffer = "";
let pending = 0;
let ended = false;
const settle = () => { if (ended && pending === 0) process.exit(0); };
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  let nl;
  while ((nl = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, nl).trim();
    buffer = buffer.slice(nl + 1);
    if (!line) continue;
    let message;
    try { message = JSON.parse(line); } catch { send({ id: null, error: { code: -32700, message: "Parse error" } }); continue; }
    pending++;
    void handle(message).finally(() => { pending--; settle(); });
  }
});
// The client closing stdin ends the server once the calls in flight have answered.
process.stdin.on("end", () => { ended = true; settle(); });
