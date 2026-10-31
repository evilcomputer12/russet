// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

import * as vscode from "vscode";

/** A tiny typed event emitter wrapper over vscode.EventEmitter. */
export class Emitter<T> implements vscode.Disposable {
  private readonly emitter = new vscode.EventEmitter<T>();
  readonly event = this.emitter.event;
  fire(value: T): void {
    this.emitter.fire(value);
  }
  dispose(): void {
    this.emitter.dispose();
  }
}

/** Dispose every item, swallowing individual errors. */
export function disposeAll(items: vscode.Disposable[]): void {
  while (items.length) {
    const d = items.pop();
    try {
      d?.dispose();
    } catch {
      /* ignore */
    }
  }
}
