// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// A tiny in-process DAP server on a Unix socket, standing in for the daemon's DAP
// hub (debug.prepare → DebugAdapterNamedPipeServer). It attaches, verifies a
// breakpoint and sends a `stopped` event so the F5 integration test can assert the
// session stops at the breakpoint. Not a real debugger — only the DAP handshake.

import * as net from "node:net";

interface DapRequest {
  seq: number;
  type: "request";
  command: string;
  arguments?: Record<string, unknown>;
}

export class MockDapServer {
  private server?: net.Server;
  constructor(public readonly socketPath: string) {}

  listen(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.server = net.createServer((socket) => this.handle(socket));
      this.server.once("error", reject);
      this.server.listen(this.socketPath, () => resolve());
    });
  }

  private handle(socket: net.Socket): void {
    let seq = 1;
    let buffer = Buffer.alloc(0);
    let stopLine = 6;

    const send = (msg: Record<string, unknown>): void => {
      const body = Buffer.from(JSON.stringify({ seq: seq++, ...msg }), "utf8");
      socket.write(`Content-Length: ${body.length}\r\n\r\n`);
      socket.write(body);
    };
    const respond = (req: DapRequest, bodyObj?: Record<string, unknown>): void => {
      send({ type: "response", request_seq: req.seq, success: true, command: req.command, body: bodyObj ?? {} });
    };
    const event = (name: string, bodyObj?: Record<string, unknown>): void => {
      send({ type: "event", event: name, body: bodyObj ?? {} });
    };

    const onRequest = (req: DapRequest): void => {
      switch (req.command) {
        case "initialize":
          respond(req, { supportsConfigurationDoneRequest: true });
          event("initialized");
          break;
        case "setBreakpoints": {
          const lines = (req.arguments?.lines as number[]) ?? [6];
          stopLine = lines[0] ?? 6;
          respond(req, { breakpoints: lines.map((line) => ({ verified: true, line })) });
          break;
        }
        case "setExceptionBreakpoints":
          respond(req, {});
          break;
        case "attach":
        case "launch":
          respond(req, {});
          break;
        case "configurationDone":
          respond(req, {});
          setTimeout(
            () => event("stopped", { reason: "breakpoint", threadId: 1, allThreadsStopped: true, line: stopLine }),
            20,
          );
          break;
        case "threads":
          respond(req, { threads: [{ id: 1, name: "main" }] });
          break;
        case "stackTrace":
          respond(req, {
            stackFrames: [
              { id: 1, name: "ContentView.body.getter", line: stopLine, column: 1, source: { name: "ContentView.swift" } },
            ],
            totalFrames: 1,
          });
          break;
        case "scopes":
          respond(req, { scopes: [{ name: "Local", variablesReference: 0, expensive: false }] });
          break;
        case "variables":
          respond(req, { variables: [] });
          break;
        case "continue":
          respond(req, { allThreadsContinued: true });
          event("continued", { threadId: 1 });
          break;
        case "disconnect":
        case "terminate":
          respond(req, {});
          event("terminated");
          break;
        default:
          respond(req, {});
      }
    };

    socket.on("data", (chunk) => {
      buffer = Buffer.concat([buffer, chunk]);
      for (;;) {
        const headerEnd = buffer.indexOf("\r\n\r\n");
        if (headerEnd < 0) {
          return;
        }
        const header = buffer.subarray(0, headerEnd).toString("utf8");
        const match = /Content-Length:\s*(\d+)/i.exec(header);
        if (!match) {
          buffer = buffer.subarray(headerEnd + 4);
          continue;
        }
        const len = Number(match[1]);
        const start = headerEnd + 4;
        if (buffer.length < start + len) {
          return;
        }
        const json = buffer.subarray(start, start + len).toString("utf8");
        buffer = buffer.subarray(start + len);
        try {
          const msg = JSON.parse(json) as DapRequest;
          if (msg.type === "request") {
            onRequest(msg);
          }
        } catch {
          /* ignore malformed frame */
        }
      }
    });
  }

  close(): void {
    this.server?.close();
  }
}
