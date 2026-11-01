// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// RussetClient: a reconnecting JSON-RPC client over the russetd Unix socket /
// named pipe, built on the generated russet/1 client (ARCHITECTURE §7). EXT-01, EXT-02.

import * as vscode from "vscode";
import * as net from "node:net";
import * as cp from "node:child_process";
import {
  createMessageConnection,
  SocketMessageReader,
  SocketMessageWriter,
  type MessageConnection,
} from "vscode-jsonrpc/node";
import {
  type Methods,
  type Notifications,
  type ClientMethods,
  type InitializeResult,
} from "../generated/protocol";
import { Emitter } from "../util/disposables";
import { defaultSocketPath, russetBinary } from "./locate";
import type { WindowsBackend, BackendRelay } from "./windowsBackend";
import { IdeRpcClient } from "./rpcClient";

export type ConnectionState =
  | "stopped"
  | "starting"
  | "connected"
  | "reconnecting"
  | "incompatible";

export interface RussetClientOptions {
  socketPath?: string;
  binDir: string;
  autoStart: boolean;
  clientVersion: string;
  backend?: WindowsBackend;
}

const PROTOCOL_MAJOR = 1;
const PROTOCOL_MINOR = 0;

/** The reconnect schedule: 0.25 s → 4 s for ~30 s (ide.md §5.4). */
const BACKOFF_MS = [250, 500, 1000, 2000, 4000];
const CONNECT_WINDOW_MS = 30_000;

export class RussetClient implements vscode.Disposable {
  private readonly log: vscode.LogOutputChannel;
  private readonly opts: RussetClientOptions;

  private socket?: net.Socket;
  private connection?: MessageConnection;
  private rpc?: IdeRpcClient;
  private backendRelay?: BackendRelay;
  private connecting?: Promise<void>;

  private _state: ConnectionState = "stopped";
  private readonly _onDidChangeState = new Emitter<ConnectionState>();
  readonly onDidChangeState = this._onDidChangeState.event;

  private _hello?: InitializeResult;
  private disposed = false;
  private startedOnce = false;

  // Handlers registered before/across connections are replayed on reconnect.
  private readonly notificationHandlers = new Map<
    keyof Notifications,
    Set<(params: unknown) => void>
  >();
  private readonly requestHandlers = new Map<
    keyof ClientMethods,
    (params: unknown) => unknown
  >();
  private readonly perConnectionDisposables: vscode.Disposable[] = [];

  constructor(opts: RussetClientOptions, log: vscode.LogOutputChannel) {
    this.opts = opts;
    this.log = log;
  }

  get state(): ConnectionState {
    return this._state;
  }

  get hello(): InitializeResult | undefined {
    return this._hello;
  }

  get socketPath(): string {
    return defaultSocketPath(this.opts.socketPath ?? "");
  }

  private setState(next: ConnectionState): void {
    if (this._state === next) {
      return;
    }
    this._state = next;
    this.log.info(`[daemon] state → ${next}`);
    this._onDidChangeState.fire(next);
  }

  /** Begin connecting; retries within the connect window, then reconnects on drop. */
  async start(): Promise<void> {
    this.startedOnce = true;
    await this.connectWithBackoff();
  }

  private connectWithBackoff(): Promise<void> {
    if (!this.connecting) {
      this.connecting = this.runConnectWithBackoff().finally(() => { this.connecting = undefined; });
    }
    return this.connecting;
  }

  private async runConnectWithBackoff(): Promise<void> {
    if (this.disposed) {
      return;
    }
    this.setState(this._hello ? "reconnecting" : "starting");
    const deadline = Date.now() + CONNECT_WINDOW_MS;
    let attempt = 0;
    let autoStartTried = false;

    while (!this.disposed && Date.now() < deadline) {
      try {
        await this.connectOnce();
        return;
      } catch (err) {
        this.log.debug(`[daemon] connect attempt ${attempt + 1} failed: ${String(err)}`);
        if (this.opts.autoStart && !this.opts.backend && !autoStartTried && attempt >= 1) {
          autoStartTried = true;
          this.tryAutoStart();
        }
        const delay = BACKOFF_MS[Math.min(attempt, BACKOFF_MS.length - 1)];
        attempt++;
        await sleep(delay);
      }
    }
    if (!this.disposed) {
      this.setState("stopped");
    }
  }

  private async connectOnce(): Promise<void> {
    const relay = await this.opts.backend?.open();
    if (this.disposed) { relay?.dispose(); return; }
    this.backendRelay = relay;
    return new Promise<void>((resolve, reject) => {
      const socket = net.connect(relay?.endpoint ?? this.socketPath);
      let settled = false;

      socket.once("error", (err) => {
        if (!settled) {
          settled = true;
          socket.destroy();
          relay?.dispose();
          reject(err);
        }
      });

      socket.once("connect", () => {
        settled = true;
        this.socket = socket;
        const connection = createMessageConnection(
          new SocketMessageReader(socket),
          new SocketMessageWriter(socket),
        );
        this.connection = connection;
        this.rpc = new IdeRpcClient(connection);

        const dropped = (reason: string): void => {
          if (this.socket === socket) this.handleDrop(reason);
        };
        connection.onClose(() => dropped("connection closed"));
        connection.onError(() => dropped("connection error"));
        socket.once("close", () => dropped("socket closed"));

        this.replayHandlers();
        connection.listen();

        this.initialize().then(resolve).catch((error: unknown) => {
          this.teardownConnection();
          reject(error);
        });
      });
    });
  }

  private async initialize(): Promise<void> {
    if (!this.rpc) {
      throw new Error("no connection");
    }
    const result = await this.rpc.request("initialize", {
      protocol: { major: PROTOCOL_MAJOR, minor: PROTOCOL_MINOR },
      client: {
        kind: "ide",
        name: "russet-vscode",
        version: this.opts.clientVersion,
        pid: process.pid,
      },
      capabilities: {
        confirm: true,
        prompt: true,
        openUrl: true,
        approvals: true,
        ideDebug: true,
        interactive: false,
      },
      workspace: {
        roots: (vscode.workspace.workspaceFolders ?? []).map((f) => f.uri.fsPath),
      },
    });
    this._hello = result;

    if (result.protocol.major !== PROTOCOL_MAJOR) {
      this.log.error(
        `[daemon] protocol major mismatch: server ${result.protocol.major}, client ${PROTOCOL_MAJOR}`,
      );
      this.setState("incompatible");
      return;
    }
    this.log.info(
      `[daemon] connected to ${result.server.name} ${result.server.version} (${result.server.buildId})`,
    );
    this.setState("connected");
  }

  private handleDrop(reason: string): void {
    if (this.disposed) {
      return;
    }
    if (this._state === "connected" || this._state === "incompatible") {
      this.log.warn(`[daemon] lost connection: ${reason}`);
    }
    this.teardownConnection();
    if (this.startedOnce) {
      void this.connectWithBackoff();
    }
  }

  private teardownConnection(): void {
    const connection = this.connection;
    const socket = this.socket;
    // Clear identity before dispose fires close/error callbacks, so an old
    // connection cannot launch another concurrent reconnect or tear down a new one.
    this.connection = undefined;
    this.socket = undefined;
    for (const d of this.perConnectionDisposables.splice(0)) {
      try {
        d.dispose();
      } catch {
        /* ignore */
      }
    }
    try {
      connection?.dispose();
    } catch {
      /* ignore */
    }
    try {
      socket?.destroy();
    } catch {
      /* ignore */
    }
    this.connection = undefined;
    this.rpc = undefined;
    this.socket = undefined;
    this.backendRelay?.dispose();
    this.backendRelay = undefined;
  }

  private replayHandlers(): void {
    if (!this.rpc) {
      return;
    }
    for (const [type, handlers] of this.notificationHandlers) {
      const d = this.rpc.onNotification(type, (params) => {
        for (const h of handlers) {
          try {
            h(params);
          } catch (err) {
            this.log.error(`[daemon] notification handler for ${String(type)} threw: ${String(err)}`);
          }
        }
      });
      this.perConnectionDisposables.push(d);
    }
    for (const [method, handler] of this.requestHandlers) {
      const d = this.rpc.onRequest(method, (params) => handler(params) as never);
      this.perConnectionDisposables.push(d);
    }
  }

  private tryAutoStart(): void {
    try {
      const bin = russetBinary(this.opts.binDir);
      this.log.info(`[daemon] auto-starting via ${bin} daemon start --detach`);
      const child = cp.spawn(bin, ["daemon", "start", "--detach"], {
        detached: true,
        stdio: "ignore",
        env: process.env,
      });
      child.on("error", (err) => this.log.warn(`[daemon] auto-start failed: ${String(err)}`));
      child.unref();
    } catch (err) {
      this.log.warn(`[daemon] auto-start error: ${String(err)}`);
    }
  }

  /** Call a daemon method (ARCHITECTURE §8). */
  async call<M extends keyof Methods>(
    method: M,
    params: Methods[M]["params"],
    token?: vscode.CancellationToken,
  ): Promise<Methods[M]["result"]> {
    if (!this.rpc) {
      throw new Error("Russet daemon is not connected.");
    }
    return this.rpc.request(method, params as Methods[M]["params"], token);
  }

  /** Subscribe to a daemon notification; survives reconnects. */
  onEvent<N extends keyof Notifications>(
    type: N,
    handler: (params: Notifications[N]) => void,
  ): vscode.Disposable {
    let set = this.notificationHandlers.get(type);
    if (!set) {
      set = new Set();
      this.notificationHandlers.set(type, set);
    }
    const typed = handler as (params: unknown) => void;
    set.add(typed);

    let live: vscode.Disposable | undefined;
    if (this.rpc) {
      live = this.rpc.onNotification(type, (params) => handler(params));
      this.perConnectionDisposables.push(live);
    }
    return {
      dispose: () => {
        set?.delete(typed);
        live?.dispose();
      },
    };
  }

  /** Answer a daemon → IDE reverse request (ui.*, ide.*); survives reconnects. */
  onRequest<M extends keyof ClientMethods>(
    method: M,
    handler: (params: ClientMethods[M]["params"]) =>
      | ClientMethods[M]["result"]
      | Promise<ClientMethods[M]["result"]>,
  ): vscode.Disposable {
    this.requestHandlers.set(method, handler as (params: unknown) => unknown);
    let live: vscode.Disposable | undefined;
    if (this.rpc) {
      live = this.rpc.onRequest(method, (params) => handler(params) as never);
      this.perConnectionDisposables.push(live);
    }
    return {
      dispose: () => {
        this.requestHandlers.delete(method);
        live?.dispose();
      },
    };
  }

  dispose(): void {
    this.disposed = true;
    this.teardownConnection();
    this._onDidChangeState.dispose();
    this.setState("stopped");
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}
