// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Russet's quieter editor: most of its defaults are the manifest's configurationDefaults, but
// application-wide settings ignore an extension's defaults. These are set once, in the user's
// settings, and only where the user has not chosen a value; changing them back sticks.

import * as vscode from "vscode";

/** Application-wide settings Russet starts with (the menu folded into the title bar). */
export const MINIMAL_APPLICATION_DEFAULTS: Readonly<Record<string, unknown>> = {
  "window.menuBarVisibility": "compact",
};

const APPLIED = "russet.minimalDefaultsApplied";

export async function applyMinimalDefaults(context: vscode.ExtensionContext): Promise<void> {
  if (context.globalState.get<boolean>(APPLIED)) {
    return;
  }
  const config = vscode.workspace.getConfiguration();
  for (const [key, value] of Object.entries(MINIMAL_APPLICATION_DEFAULTS)) {
    if (config.inspect(key)?.globalValue === undefined) {
      // One that this editor version does not register is skipped, not fatal.
      await Promise.resolve(config.update(key, value, vscode.ConfigurationTarget.Global)).catch(() => undefined);
    }
  }
  await context.globalState.update(APPLIED, true);
}
