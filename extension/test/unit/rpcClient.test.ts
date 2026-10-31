// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { it, expect } from "vitest";
import { PassThrough } from "node:stream";
import { createMessageConnection, StreamMessageReader, StreamMessageWriter, CancellationTokenSource, type RequestMessage } from "vscode-jsonrpc/node";
import { IdeRpcClient } from "../../src/daemon/rpcClient";

it.each([false, true])("sends object params with cancellation token present=%s", async (withToken) => {
  const incoming = new PassThrough();
  const outgoing = new PassThrough();
  const connection = createMessageConnection(new StreamMessageReader(incoming), new StreamMessageWriter(outgoing));
  connection.listen();
  const source = new CancellationTokenSource();
  let received: unknown;
  const reader = new StreamMessageReader(outgoing);
  const listener = reader.listen((message) => {
    const request = message as RequestMessage;
    received = request.params;
    const body = Buffer.from(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: {} }));
    incoming.write(`Content-Length: ${body.length}\r\n\r\n`);
    incoming.write(body);
  });
  try {
    const rpc = new IdeRpcClient(connection);
    await rpc.request("daemon.ping", {}, withToken ? source.token : undefined);
    expect(received).toEqual({});
    expect(Array.isArray(received)).toBe(false);
  } finally {
    listener.dispose(); reader.dispose(); connection.dispose(); source.dispose();
    incoming.destroy(); outgoing.destroy();
  }
});
