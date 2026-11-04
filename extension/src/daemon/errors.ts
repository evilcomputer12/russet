// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Turns a daemon ErrorData (ARCHITECTURE §13) into a user-facing notification.
// ide.md §8.

import * as vscode from "vscode";
import { ResponseError } from "vscode-jsonrpc/node";
import type { ErrorData } from "../generated/protocol";
import { openErrorDocs, windowsPaths } from "../util/docs";
import { explain } from "../run/explain";

export function errorDataOf(err: unknown): ErrorData | undefined {
  if (err instanceof ResponseError && err.data && typeof err.data === "object") {
    const data = err.data as Partial<ErrorData>;
    if (typeof data.russetCode === "string") {
      return data as ErrorData;
    }
  }
  return undefined;
}

/** A request the user cancelled (`-32800`, `core.cancelled`, ARCHITECTURE §7.6). */
export function isCancelled(err: unknown): boolean {
  return (err instanceof ResponseError && err.code === -32800) || errorDataOf(err)?.russetCode === "core.cancelled";
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

/** An extra button on an error notification, such as "Show Build Log". */
export interface ErrorAction {
  label: string;
  run: () => unknown;
}

/** Show a daemon error as a notification with a Docs button when available, plus any `actions`. */
export async function showDaemonError(err: unknown, context: string, actions: ErrorAction[] = []): Promise<void> {
  const data = errorDataOf(err);
  // The backend's paths as Windows shows them (/mnt/c/… → C:\…).
  const base = windowsPaths(messageOf(err));
  const labels = actions.map((a) => a.label);
  if (!data) {
    const picked = await vscode.window.showErrorMessage(`${context}: ${base}`, ...labels);
    await actions.find((a) => a.label === picked)?.run();
    return;
  }
  // A compiler error the IDE can say in plain words says that rather than the generic hint.
  const meaning = explain(/: error: (.+)$/.exec(base)?.[1] ?? "");
  const hint = meaning ?? (data.hint ? windowsPaths(data.hint) : undefined);
  const detail = hint ? `${base} — ${hint}` : base;
  if (data.docs) {
    labels.push("Open Docs");
  }
  const choice = await vscode.window.showErrorMessage(
    `${context}: ${detail} (${data.russetCode})`,
    ...labels,
  );
  if (choice === "Open Docs" && data.docs) {
    // The error's section of Russet's bundled error reference (docs/errors.md).
    await openErrorDocs(data.docs);
    return;
  }
  await actions.find((a) => a.label === choice)?.run();
}
