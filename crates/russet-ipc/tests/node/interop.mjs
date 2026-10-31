// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

// Stock vscode-jsonrpc 9.0.3 (SocketMessageReader/Writer) against a russet-ipc server on a Unix socket
// (ARCHITECTURE §7.2; PLAN W2 tests): initialize, $/progress, cancel -> -32800, unknown method -> -32601, and
// a ui.confirm request answered by this client. Prints one JSON summary line and exits 0 when every check
// passed. Usage: node interop.mjs <socket path>

import net from 'node:net';
import { once } from 'node:events';
import {
  CancellationTokenSource,
  ProgressType,
  SocketMessageReader,
  SocketMessageWriter,
  createMessageConnection,
} from 'vscode-jsonrpc/node';

const socketPath = process.argv[2];
const results = {};
const fail = (msg) => {
  console.log(JSON.stringify({ ok: false, error: msg, results }));
  process.exit(1);
};

const socket = net.connect(socketPath);
await once(socket, 'connect');
const conn = createMessageConnection(new SocketMessageReader(socket), new SocketMessageWriter(socket));
let confirms = 0;
conn.onRequest('ui.confirm', (params) => {
  confirms += 1;
  results.confirmTitle = params.title;
  return { confirmed: true };
});
conn.listen();

// initialize
const init = await conn.sendRequest('initialize', {
  protocol: { major: 1, minor: 0 },
  client: { kind: 'ide', name: 'vscode-jsonrpc-interop', version: '9.0.3', pid: process.pid },
  capabilities: { confirm: true, prompt: false, openUrl: false, approvals: false, ideDebug: false, interactive: false },
});
if (!init.connectionId || !init.connectionId.startsWith('c-')) fail(`bad initialize result ${JSON.stringify(init)}`);
results.initialize = init.connectionId;

// $/progress and cancellation -> -32800
const progress = new ProgressType();
const token = 'interop-progress-1';
const seen = [];
const cts = new CancellationTokenSource();
const disposable = conn.onProgress(progress, token, (value) => {
  seen.push(value.kind);
  if (seen.filter((k) => k === 'report').length >= 2) cts.cancel();
});
try {
  await conn.sendRequest('test.slow', { steps: 2, progressToken: token }, cts.token);
  fail('test.slow returned instead of being cancelled');
} catch (e) {
  results.cancelCode = e.code;
}
disposable.dispose();
results.progress = seen;
if (results.cancelCode !== -32800) fail(`cancel gave ${results.cancelCode}`);
if (seen[0] !== 'begin' || seen.filter((k) => k === 'report').length < 2) fail(`progress ${JSON.stringify(seen)}`);

// unknown method -> -32601
try {
  await conn.sendRequest('test.nope', {});
  fail('an unknown method succeeded');
} catch (e) {
  results.unknownCode = e.code;
}
if (results.unknownCode !== -32601) fail(`unknown method gave ${results.unknownCode}`);

// a confirmation answered by this client
const confirmed = await conn.sendRequest('test.confirm', {});
results.confirm = confirmed;
if (confirms !== 1) fail(`ui.confirm was asked ${confirms} times`);

console.log(JSON.stringify({ ok: true, results }));
conn.dispose();
socket.destroy();
process.exit(0);
