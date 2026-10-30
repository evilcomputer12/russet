// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

import * as vscode from "vscode";

let channel: vscode.LogOutputChannel | undefined;

/** The shared Russet log output channel (created once). */
export function log(): vscode.LogOutputChannel {
  if (!channel) {
    channel = vscode.window.createOutputChannel("Russet", { log: true });
  }
  return channel;
}

export function disposeLog(): void {
  channel?.dispose();
  channel = undefined;
}
