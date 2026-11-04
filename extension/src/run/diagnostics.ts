// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The "russet" DiagnosticCollection, fed by job.events diagnostics (EXT-05).

import * as vscode from "vscode";
import type { JobEvent } from "../generated/protocol";
import { windowsPaths } from "../util/docs";
import { explain } from "./explain";

type DiagnosticEvent = Extract<JobEvent, { kind: "diagnostic" }>;

export class Diagnostics implements vscode.Disposable {
  private readonly collection = vscode.languages.createDiagnosticCollection("russet");
  private readonly byFile = new Map<string, vscode.Diagnostic[]>();

  /** Clear everything (start of a build). */
  reset(): void {
    this.byFile.clear();
    this.collection.clear();
  }

  add(event: DiagnosticEvent): void {
    if (!event.file) {
      return;
    }
    // The relay converts only path fields it knows; a diagnostic's file comes as WSL sees it (/mnt/c/…).
    const uri = vscode.Uri.file(windowsPaths(event.file));
    const line = Math.max(0, (event.line ?? 1) - 1);
    const column = Math.max(0, (event.column ?? 1) - 1);
    const range = new vscode.Range(line, column, line, column + 1);
    // The compiler's message, then what it means in plain words for the common errors.
    const meaning = explain(event.message);
    const diag = new vscode.Diagnostic(range, meaning ? `${event.message}
${meaning}` : event.message, severityOf(event.severity));
    diag.source = event.tool ?? "russet";
    if (event.code) {
      diag.code = event.code;
    }
    const key = uri.toString();
    const list = this.byFile.get(key) ?? [];
    list.push(diag);
    this.byFile.set(key, list);
    this.collection.set(uri, list);
  }

  get errorCount(): number {
    let n = 0;
    for (const list of this.byFile.values()) {
      n += list.filter((d) => d.severity === vscode.DiagnosticSeverity.Error).length;
    }
    return n;
  }

  dispose(): void {
    this.collection.dispose();
  }
}

function severityOf(sev: DiagnosticEvent["severity"]): vscode.DiagnosticSeverity {
  switch (sev) {
    case "error":
      return vscode.DiagnosticSeverity.Error;
    case "warning":
      return vscode.DiagnosticSeverity.Warning;
    default:
      return vscode.DiagnosticSeverity.Information;
  }
}
