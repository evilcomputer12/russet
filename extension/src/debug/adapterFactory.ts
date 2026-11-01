// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The debug adapter descriptor factory for the `russet` type (EXT-06). The prepared
// adapter (from debug.prepare) is stashed by configProvider under a token; here we
// turn it into either a named-pipe connection to the daemon's DAP hub
// (DebugAdapterNamedPipeServer, ide.md §5.7) or an lldb-dap executable fallback.

import * as vscode from "vscode";
import type { DebugPrepareResult } from "../generated/protocol";
import type { WindowsBackend, BackendRelay } from "../daemon/windowsBackend";

const prepared = new Map<string, DebugPrepareResult>();
let counter = 0;

/** Register a prepared session; returns the token to embed in the configuration. */
export function stashPrepared(result: DebugPrepareResult): string {
  const token = `russet-dap-${++counter}`;
  prepared.set(token, result);
  return token;
}

export function takePrepared(token: string | undefined): DebugPrepareResult | undefined {
  if (!token) {
    return undefined;
  }
  return prepared.get(token);
}

export function dropPrepared(token: string | undefined): void {
  if (token) {
    prepared.delete(token);
  }
}

export class RussetDebugAdapterFactory implements vscode.DebugAdapterDescriptorFactory {
  private readonly relays = new Map<string, BackendRelay>();

  constructor(private readonly backend?: WindowsBackend) {}

  release(sessionId: string): void {
    this.relays.get(sessionId)?.dispose();
    this.relays.delete(sessionId);
  }

  dispose(): void {
    for (const relay of this.relays.values()) relay.dispose();
    this.relays.clear();
  }

  async createDebugAdapterDescriptor(
    session: vscode.DebugSession,
  ): Promise<vscode.DebugAdapterDescriptor> {
    // The virtual iPhone: lldb-dap in the backend, through the relay that maps Windows and Linux paths.
    if (session.configuration.type === "russet-virtual") {
      const socket = (session.configuration as { __dapSocket?: unknown }).__dapSocket;
      if (!this.backend || typeof socket !== "string") {
        throw new Error("Russet: debugging the virtual iPhone needs Russet's Windows backend.");
      }
      this.release(session.id);
      const relay = await this.backend.open(socket, "dap");
      this.relays.set(session.id, relay);
      return new vscode.DebugAdapterNamedPipeServer(relay.endpoint);
    }
    const token = (session.configuration as { __russetToken?: string }).__russetToken;
    const result = takePrepared(token);
    if (!result) {
      throw new Error("Russet: no prepared debug session (debug.prepare did not run).");
    }

    // Preferred: the daemon's DAP hub over a named pipe / unix socket.
    if (result.dap) {
      if (this.backend && result.dap.transport === "unix") {
        this.release(session.id);
        const relay = await this.backend.open(result.dap.path, "dap");
        this.relays.set(session.id, relay);
        return new vscode.DebugAdapterNamedPipeServer(relay.endpoint);
      }
      if (process.platform === "win32" && result.dap.transport === "unix") {
        throw new Error("Russet: the Linux debugger endpoint needs a managed Windows backend relay.");
      }
      return new vscode.DebugAdapterNamedPipeServer(result.dap.path);
    }

    // Fallback: launch lldb-dap directly with the environment the daemon reported.
    if (this.backend) {
      throw new Error("Russet: the managed backend did not provide a DAP hub endpoint.");
    }
    const tool = result.lldbDap;
    const configured = vscode.workspace
      .getConfiguration("russet")
      .get<string>("debug.lldbDapPath", "");
    const command = configured || tool.path;
    return new vscode.DebugAdapterExecutable(command, [], { env: tool.env });
  }
}
