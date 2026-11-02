// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Runs inside the installed editor (@vscode/test-electron). It activates the
// development copy of the Russet extension, checks it connected and contributed
// the W19 commands, then executes the commands the outer driver
// (run-installed.mjs) writes as cmd-<n>.json in SMOKE_DIR, acknowledging each
// with ack-<n>.json, until it receives "__done".

/* global require, process, exports, setTimeout */
/* eslint-disable @typescript-eslint/no-require-imports */
"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vscode = require("vscode");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

exports.run = async () => {
  const dir = process.env.SMOKE_DIR;
  assert.ok(dir, "SMOKE_DIR is set");
  const extension = vscode.extensions.getExtension("russet.russet");
  assert.ok(extension, "the Russet extension is installed");
  const api = await extension.activate();
  const deadline = Date.now() + 45000;
  while (api.client.state !== "connected" && Date.now() < deadline) await sleep(100);
  const commands = await vscode.commands.getCommands(true);
  const required = [
    "russet.openSetup", "russet.setup.openWalkthrough", "russet.account.signInAppleId", "russet.openVirtualIphone",
    "russet.selectDevice", "russet.virtual.home", "russet.virtual.lock", "russet.virtual.volumeUp",
    "russet.virtual.volumeDown", "russet.virtual.rotateLeft", "russet.virtual.rotateRight", "russet.virtual.screenshot",
  ];
  const missing = required.filter((c) => !commands.includes(c));
  fs.writeFileSync(path.join(dir, "ready.json"), JSON.stringify({
    appName: vscode.env.appName,
    extensionPath: extension.extensionPath,
    state: api.client.state,
    missing,
  }));
  assert.deepEqual(missing, []);

  for (let n = 1; ; n++) {
    const file = path.join(dir, `cmd-${n}.json`);
    while (!fs.existsSync(file)) await sleep(100);
    await sleep(50);
    const cmd = JSON.parse(fs.readFileSync(file, "utf8"));
    if (cmd.command === "__done") {
      fs.writeFileSync(path.join(dir, `ack-${n}.json`), JSON.stringify({ ok: true }));
      return;
    }
    let ack;
    try {
      if (cmd.command === "__setting") {
        await vscode.workspace.getConfiguration().update(cmd.args[0], cmd.args[1], vscode.ConfigurationTarget.Global);
        ack = { ok: true };
      } else if (cmd.command === "__state") {
        ack = { ok: true, result: { state: api.client.state } };
      } else {
        // Commands that open a quick pick resolve only when it closes: do not wait.
        const p = vscode.commands.executeCommand(cmd.command, ...(cmd.args || []));
        const result = cmd.noWait ? undefined : await p;
        ack = { ok: true, result: result === undefined ? null : result };
      }
    } catch (err) {
      ack = { ok: false, error: String(err && err.message ? err.message : err) };
    }
    fs.writeFileSync(path.join(dir, `ack-${n}.json`), JSON.stringify(ack));
  }
};
