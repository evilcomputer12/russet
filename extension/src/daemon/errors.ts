// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Turns a daemon ErrorData (ARCHITECTURE §13) into a user-facing notification.
// ide.md §8.

import * as vscode from "vscode";
import { ResponseError } from "vscode-jsonrpc/node";
import type { ErrorData } from "../generated/protocol";

export function errorDataOf(err: unknown): ErrorData | undefined {
  if (err instanceof ResponseError && err.data && typeof err.data === "object") {
    const data = err.data as Partial<ErrorData>;
    if (typeof data.russetCode === "string") {
      return data as ErrorData;
    }
  }
  return undefined;
}

export function messageOf(err: unknown): string {
  if (err instanceof ResponseError) {
    return err.message;
  }
  if (err instanceof Error) {
    return err.message;
  }
  return String(err);
}

/** Show a daemon error as a notification with a Docs button when available. */
export async function showDaemonError(err: unknown, context: string): Promise<void> {
  const data = errorDataOf(err);
  const base = messageOf(err);
  if (!data) {
    await vscode.window.showErrorMessage(`${context}: ${base}`);
    return;
  }
  const detail = data.hint ? `${base} — ${data.hint}` : base;
  const buttons: string[] = [];
  if (data.docs) {
    buttons.push("Open Docs");
  }
  const choice = await vscode.window.showErrorMessage(
    `${context}: ${detail} (${data.russetCode})`,
    ...buttons,
  );
  if (choice === "Open Docs" && data.docs) {
    await vscode.env.openExternal(vscode.Uri.parse(data.docs));
  }
}
