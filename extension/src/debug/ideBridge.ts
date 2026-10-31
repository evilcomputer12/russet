// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Handles the daemon → IDE reverse requests (ARCHITECTURE §9.3, §9.4): the ui.*
// prompts and the ide.* debug/breakpoint/reveal requests an MCP agent drives while
// the IDE is open (ide.md §5.4). Breakpoints an agent sets appear in VS Code's list.

import * as vscode from "vscode";
import type { RussetClient } from "../daemon/client";

export function registerIdeBridge(
  client: RussetClient,
  disposables: vscode.Disposable[],
): void {
  disposables.push(
    client.onRequest("ui.confirm", async (p) => {
      const button = p.danger ? "Delete" : "OK";
      const choice = await vscode.window.showWarningMessage(
        p.message,
        { modal: true, detail: p.detail },
        button,
      );
      return { confirmed: choice === button };
    }),
  );

  disposables.push(
    client.onRequest("ui.prompt", async (p) => {
      if (p.kind === "typed-confirmation") {
        const value = await vscode.window.showInputBox({
          title: p.title,
          prompt: `${p.message} (type "${p.expected ?? ""}" to confirm)`,
          validateInput: (v) => (v === p.expected ? undefined : "Does not match."),
        });
        return { value, cancelled: value === undefined };
      }
      const value = await vscode.window.showInputBox({
        title: p.title,
        prompt: p.message,
        password: p.secret || p.kind === "password",
        validateInput:
          p.kind === "code"
            ? (v) => (/^\d{6}$/.test(v) ? undefined : "Enter the 6-digit code.")
            : undefined,
      });
      return { value, cancelled: value === undefined };
    }),
  );

  disposables.push(
    client.onRequest("ui.openUrl", async (p) => {
      const opened = await vscode.env.openExternal(vscode.Uri.parse(p.url));
      return { opened };
    }),
  );

  // ide.* requests carry unspecified payloads in the generated contract (W11 owns
  // the detailed shapes); implement them structurally.
  disposables.push(
    client.onRequest("ide.reveal", async (p) => {
      const params = p as { file?: string; line?: number; column?: number };
      if (params.file) {
        const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(params.file));
        const line = Math.max(0, (params.line ?? 1) - 1);
        const pos = new vscode.Position(line, Math.max(0, (params.column ?? 1) - 1));
        await vscode.window.showTextDocument(doc, { selection: new vscode.Range(pos, pos) });
      }
      return {};
    }),
  );

  disposables.push(
    client.onRequest("ide.breakpoints.add", async (p) => {
      const params = p as { file?: string; line?: number; condition?: string; logMessage?: string };
      if (!params.file) {
        return { id: "" };
      }
      const pos = new vscode.Position(Math.max(0, (params.line ?? 1) - 1), 0);
      const bp = new vscode.SourceBreakpoint(
        new vscode.Location(vscode.Uri.file(params.file), pos),
        true,
        params.condition,
        undefined,
        params.logMessage,
      );
      vscode.debug.addBreakpoints([bp]);
      return { id: bp.id };
    }),
  );

  disposables.push(
    client.onRequest("ide.breakpoints.remove", async (p) => {
      const params = p as { id?: string };
      const bp = vscode.debug.breakpoints.find((b) => b.id === params.id);
      if (bp) {
        vscode.debug.removeBreakpoints([bp]);
      }
      return {};
    }),
  );

  disposables.push(
    client.onRequest("ide.debug.start", async (p) => {
      const params = p as { configuration?: vscode.DebugConfiguration };
      const folder = vscode.workspace.workspaceFolders?.[0];
      const config = params.configuration ?? { type: "russet", request: "launch", name: "Debug on iPhone" };
      const ok = await vscode.debug.startDebugging(folder, config);
      return { started: ok, sessionId: vscode.debug.activeDebugSession?.id ?? "" };
    }),
  );
}
