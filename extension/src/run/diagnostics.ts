// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The "russet" DiagnosticCollection, fed by job.events diagnostics (EXT-05).

import * as vscode from "vscode";
import type { JobEvent } from "../generated/protocol";

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
    const uri = vscode.Uri.file(event.file);
    const line = Math.max(0, (event.line ?? 1) - 1);
    const column = Math.max(0, (event.column ?? 1) - 1);
    const range = new vscode.Range(line, column, line, column + 1);
    const diag = new vscode.Diagnostic(range, event.message, severityOf(event.severity));
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
