// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { ParameterStructures, type CancellationToken } from "vscode-jsonrpc/node";
import { RussetClient as GeneratedClient, type LongRunningOptions, type Methods } from "../generated/protocol";

// vscode-jsonrpc 8 treats an explicit undefined third argument as a second
// positional parameter. Keep the generated DTOs/client intact and enforce the
// russet/1 named-object contract at the IDE's library boundary.
export class IdeRpcClient extends GeneratedClient {
  override request<M extends keyof Methods>(
    method: M,
    params: Methods[M]["params"] & LongRunningOptions,
    token?: CancellationToken,
  ): Promise<Methods[M]["result"]> {
    return (token === undefined
      ? this.connection.sendRequest(method, ParameterStructures.byName, params)
      : this.connection.sendRequest(method, ParameterStructures.byName, params, token)) as Promise<Methods[M]["result"]>;
  }
}
