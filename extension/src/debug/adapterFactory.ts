// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The debug adapter descriptor factory for the `russet` type (EXT-06). The prepared
// adapter (from debug.prepare) is stashed by configProvider under a token; here we
// turn it into either a named-pipe connection to the daemon's DAP hub
// (DebugAdapterNamedPipeServer, ide.md §5.7) or an lldb-dap executable fallback.

import * as vscode from "vscode";
import type { DebugPrepareResult } from "../generated/protocol";

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
  createDebugAdapterDescriptor(
    session: vscode.DebugSession,
  ): vscode.ProviderResult<vscode.DebugAdapterDescriptor> {
    const token = (session.configuration as { __russetToken?: string }).__russetToken;
    const result = takePrepared(token);
    if (!result) {
      throw new Error("Russet: no prepared debug session (debug.prepare did not run).");
    }

    // Preferred: the daemon's DAP hub over a named pipe / unix socket.
    if (result.dap) {
      return new vscode.DebugAdapterNamedPipeServer(result.dap.path);
    }

    // Fallback: launch lldb-dap directly with the environment the daemon reported.
    const tool = result.lldbDap;
    const configured = vscode.workspace
      .getConfiguration("russet")
      .get<string>("debug.lldbDapPath", "");
    const command = configured || tool.path;
    return new vscode.DebugAdapterExecutable(command, [], { env: tool.env });
  }
}
