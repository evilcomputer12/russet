// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// A self-contained fake russetd for the extension integration suite. It speaks the
// russet/1 protocol (ARCHITECTURE §7) over a Unix socket and answers just enough for
// the connect + device + F5-debug flow. This stands in for W15's QA-15 mock-daemon
// backend, which has not merged yet; swap it out once QA-15 lands.

import * as net from "node:net";
import * as fs from "node:fs";
import * as path from "node:path";
import {
  createMessageConnection,
  SocketMessageReader,
  SocketMessageWriter,
  type MessageConnection,
} from "vscode-jsonrpc/node";
import { MockDapServer } from "./mock-dap";

type Handler = (params: Record<string, unknown>) => unknown;

export interface FakeDaemonOptions {
  socketPath: string;
  projectRoot: string;
}

export class FakeDaemon {
  private server?: net.Server;
  private readonly dapServers: MockDapServer[] = [];
  private readonly connections: MessageConnection[] = [];

  constructor(private readonly opts: FakeDaemonOptions) {}

  async listen(): Promise<void> {
    fs.mkdirSync(path.dirname(this.opts.socketPath), { recursive: true });
    if (fs.existsSync(this.opts.socketPath)) {
      fs.rmSync(this.opts.socketPath);
    }
    await new Promise<void>((resolve, reject) => {
      this.server = net.createServer((socket) => this.onConnection(socket));
      this.server.once("error", reject);
      this.server.listen(this.opts.socketPath, () => resolve());
    });
  }

  private onConnection(socket: net.Socket): void {
    const connection = createMessageConnection(
      new SocketMessageReader(socket),
      new SocketMessageWriter(socket),
    );
    this.connections.push(connection);
    const handlers = this.handlers();
    for (const [method, handler] of Object.entries(handlers)) {
      connection.onRequest(method, (params: unknown) => handler((params ?? {}) as Record<string, unknown>));
    }
    connection.listen();
  }

  private handlers(): Record<string, Handler> {
    const root = this.opts.projectRoot;
    return {
      initialize: () => ({
        protocol: { major: 1, minor: 0 },
        server: {
          name: "russetd",
          version: "0.0.0-fake",
          buildId: "0.0.0+gfake",
          pid: process.pid,
          executable: "fake-russetd",
          executableId: { dev: 0, ino: 0, mtimeNs: 0, size: 0 },
          startedAt: new Date().toISOString(),
          platform: process.platform,
        },
        connectionId: "c-1",
        grant: { profile: "ide", allow: [], confirm: [], approval: [], deny: [] },
        capabilities: { methods: [], notifications: [], features: { mirror: false, wifiDevices: false, mcpHttp: false } },
        endpoints: {},
      }),
      "device.watch": () => ({ subscriptionId: "sub-dev" }),
      "device.list": () => ({
        devices: [
          {
            udid: "00008101-FAKEFAKEFAKE",
            name: "Test iPhone",
            marketingName: "iPhone 12",
            productType: "iPhone13,2",
            osVersion: "27.0",
            build: "24A437",
            transport: "usb",
            codeSigningMonitor: "txm",
            state: "ready",
          },
        ],
      }),
      "job.subscribe": () => ({ subscriptionId: "sub-job" }),
      "run.subscribe": () => ({ subscriptionId: "sub-run" }),
      "account.list": () => ({ accounts: [] }),
      "project.info": () => ({
        stack: "swiftpm",
        root,
        name: "Hello",
        targets: [{ name: "Hello", bundleId: "com.example.Hello", kind: "app" }],
        configurations: ["debug", "release"],
        defaultTarget: "Hello",
      }),
      "debug.prepare": async () => {
        const dapSock = path.join(path.dirname(this.opts.socketPath), `dap-${this.dapServers.length}.sock`);
        if (fs.existsSync(dapSock)) {
          fs.rmSync(dapSock);
        }
        const dap = new MockDapServer(dapSock);
        await dap.listen();
        this.dapServers.push(dap);
        return {
          sessionId: "sess-1",
          pid: 4242,
          bundleId: "com.example.Hello",
          lldb: fakeTool("lldb"),
          lldbDap: fakeTool("lldb-dap"),
          broker: { transport: "unix", path: "/tmp/fake-gdb.sock", lldbUrl: "unix-connect:///tmp/fake-gdb.sock" },
          initCommands: ["settings set target.inherit-env false"],
          attachCommands: ["process attach --pid 4242"],
          dapConfig: {},
          dap: { transport: "unix", path: dapSock },
          attachMode: "broker",
        };
      },
      "run.stop": () => ({}),
      "run.start": () => ({ sessionId: "sess-run", pid: 4243, bundleId: "com.example.Hello", installed: true }),
    };
  }

  dispose(): void {
    for (const dap of this.dapServers) {
      dap.close();
    }
    for (const c of this.connections) {
      try {
        c.dispose();
      } catch {
        /* ignore */
      }
    }
    this.server?.close();
    try {
      if (fs.existsSync(this.opts.socketPath)) {
        fs.rmSync(this.opts.socketPath);
      }
    } catch {
      /* ignore */
    }
  }
}

function fakeTool(component: string): Record<string, unknown> {
  return {
    component,
    version: "17.0.0",
    path: `/usr/bin/${component}`,
    provenance: { kind: "system" },
    env: {},
    ok: true,
    problems: [],
  };
}
