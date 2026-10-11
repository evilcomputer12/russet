// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

// Russet's Node interop harness (CORE-15; core §19 "Interop", §23 #12): a stock vscode-jsonrpc 9.0.3 client,
// as the VS Code extension uses it, driving a russet/1 daemon over its Unix socket (ARCHITECTURE §7.1–§7.6).
//
//   node interop.mjs <plan.json>
//
// The plan (written by russet_testkit::node::InteropPlan) names the socket, the `initialize` params and the
// steps; the report on stdout is one JSON object: {"ok": bool, "steps": [{"name", "kind", "ok", ...}]}.
// Step kinds:
//   request              sendRequest(method, params); ok when it succeeds
//   unknownMethod        sendRequest(method); ok when the answer is JSON-RPC -32601
//   cancelAfterProgress  sendRequest(method, {...params, progressToken}); after the first $/progress value,
//                        cancel through a CancellationTokenSource; ok when the answer is -32800
//   confirm              sendRequest(method, params) while answering ui.confirm with plan.confirmAnswer; ok when
//                        a ui.confirm arrived and the result's `confirmed` equals the answer's

import fs from 'node:fs';
import net from 'node:net';
import rpc from 'vscode-jsonrpc/node';

const { createMessageConnection, SocketMessageReader, SocketMessageWriter, CancellationTokenSource, ProgressType } =
  rpc;

const METHOD_NOT_FOUND = -32601;
const REQUEST_CANCELLED = -32800;

function withTimeout(promise, ms, what) {
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`timeout after ${ms} ms: ${what}`)), ms);
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
}

function describeError(e) {
  return { code: e?.code, message: e?.message, data: e?.data };
}

async function main() {
  const plan = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
  const timeoutMs = plan.timeoutMs ?? 10000;
  const socket = net.createConnection(plan.socket);
  await withTimeout(
    new Promise((resolve, reject) => {
      socket.once('connect', resolve);
      socket.once('error', reject);
    }),
    timeoutMs,
    `connecting to ${plan.socket}`,
  );
  const connection = createMessageConnection(new SocketMessageReader(socket), new SocketMessageWriter(socket));
  const confirms = [];
  connection.onRequest('ui.confirm', (params) => {
    confirms.push(params);
    return plan.confirmAnswer ?? { confirmed: true };
  });
  connection.listen();

  const report = { ok: true, steps: [] };
  const record = (step) => {
    report.steps.push(step);
    if (!step.ok) report.ok = false;
  };

  try {
    const result = await withTimeout(connection.sendRequest('initialize', plan.initialize), timeoutMs, 'initialize');
    record({ name: 'initialize', kind: 'initialize', ok: true, result });
  } catch (e) {
    record({ name: 'initialize', kind: 'initialize', ok: false, error: describeError(e) });
  }

  let tokens = 0;
  for (const step of plan.steps ?? []) {
    const base = { name: step.name, kind: step.kind };
    try {
      switch (step.kind) {
        case 'request': {
          const result = await withTimeout(connection.sendRequest(step.method, step.params ?? {}), timeoutMs, step.method);
          record({ ...base, ok: true, result });
          break;
        }
        case 'unknownMethod': {
          try {
            const result = await withTimeout(connection.sendRequest(step.method, {}), timeoutMs, step.method);
            record({ ...base, ok: false, result, detail: 'expected an error' });
          } catch (e) {
            record({ ...base, ok: e?.code === METHOD_NOT_FOUND, error: describeError(e) });
          }
          break;
        }
        case 'cancelAfterProgress': {
          tokens += 1;
          const token = `interop-${tokens}`;
          const progress = [];
          let firstProgress;
          const sawProgress = new Promise((resolve) => {
            firstProgress = resolve;
          });
          const sub = connection.onProgress(new ProgressType(), token, (value) => {
            progress.push(value);
            firstProgress();
          });
          const cts = new CancellationTokenSource();
          const pending = connection.sendRequest(step.method, { ...(step.params ?? {}), progressToken: token }, cts.token);
          try {
            await withTimeout(sawProgress, timeoutMs, `$/progress of ${step.method}`);
            cts.cancel();
            const result = await withTimeout(pending, timeoutMs, `${step.method} after cancel`);
            record({ ...base, ok: false, progress, result, detail: 'expected -32800 after cancel' });
          } catch (e) {
            record({ ...base, ok: e?.code === REQUEST_CANCELLED && progress.length > 0, progress, error: describeError(e) });
          } finally {
            sub.dispose();
          }
          break;
        }
        case 'confirm': {
          const before = confirms.length;
          const result = await withTimeout(connection.sendRequest(step.method, step.params ?? {}), timeoutMs, step.method);
          const want = (plan.confirmAnswer ?? { confirmed: true }).confirmed;
          const asked = confirms.length > before;
          record({ ...base, ok: asked && result?.confirmed === want, result, confirm: confirms[confirms.length - 1] });
          break;
        }
        default:
          record({ ...base, ok: false, detail: `unknown step kind ${step.kind}` });
      }
    } catch (e) {
      record({ ...base, ok: false, error: describeError(e) });
    }
  }

  connection.dispose();
  socket.end();
  process.stdout.write(`${JSON.stringify(report)}\n`);
  process.exitCode = report.ok ? 0 : 1;
}

main().catch((e) => {
  process.stdout.write(`${JSON.stringify({ ok: false, steps: [], fatal: String(e?.stack ?? e) })}\n`);
  process.exitCode = 2;
});
