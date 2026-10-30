// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// A minimal `vscode` stand-in for vitest unit tests (no Electron). Only the surface
// the pure-logic modules touch is implemented.

export class EventEmitter<T> {
  private listeners: Array<(e: T) => void> = [];
  event = (listener: (e: T) => void): { dispose(): void } => {
    this.listeners.push(listener);
    return {
      dispose: () => {
        this.listeners = this.listeners.filter((l) => l !== listener);
      },
    };
  };
  fire(value: T): void {
    for (const l of [...this.listeners]) {
      l(value);
    }
  }
  dispose(): void {
    this.listeners = [];
  }
}

export const window = {
  createOutputChannel: () => ({
    info: () => {},
    warn: () => {},
    error: () => {},
    debug: () => {},
    show: () => {},
    dispose: () => {},
  }),
  showErrorMessage: async () => undefined,
  showInformationMessage: async () => undefined,
  showWarningMessage: async () => undefined,
};

export const Uri = {
  file: (p: string) => ({ fsPath: p, toString: () => `file://${p}` }),
  parse: (s: string) => ({ toString: () => s }),
};

export const env = {
  openExternal: async () => true,
};

export const commands = {
  executeCommand: async () => undefined,
};

export const workspace = {
  workspaceFolders: undefined as unknown,
  getConfiguration: () => ({ get: (_k: string, d?: unknown) => d }),
};
