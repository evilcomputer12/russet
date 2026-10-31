// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Bundle identifiers for new and existing apps. Apple registers an identifier
// for one developer only and never registers the com.example.* placeholders,
// so the IDE asks once for the user's own reverse-DNS prefix (the
// "organization identifier", as Xcode calls it), saves it in
// `russet.project.organizationIdentifier`, and uses it to name new projects
// and to replace a template's placeholder before the first run.

import * as vscode from "vscode";

export const ORGANIZATION_SETTING = "russet.project.organizationIdentifier";

const PREFIX = /^[A-Za-z][A-Za-z0-9-]*(\.[A-Za-z0-9-]+)+$/;
const BUNDLE_ID = /^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$/;
const PLACEHOLDER = /^(com|org|net)\.example(\.|$)/i;

/** Whether `bundleId` is a placeholder Apple never registers (`com.example.*` and the like). */
export function isPlaceholder(bundleId: string): boolean {
  return PLACEHOLDER.test(bundleId.trim());
}

/** Why `value` cannot be an organization identifier, or undefined when it can. */
export function prefixProblem(value: string): string | undefined {
  const v = value.trim();
  if (!PREFIX.test(v)) {
    return "Use reverse-DNS: letters, digits and hyphens, with at least one dot, like com.yourname.";
  }
  if (isPlaceholder(v)) {
    return "com.example is a placeholder Apple never registers. Use your own, like com.yourname.";
  }
  return undefined;
}

/** Why `value` cannot be a bundle identifier, or undefined when it can. */
export function bundleIdProblem(value: string): string | undefined {
  const v = value.trim();
  if (!BUNDLE_ID.test(v)) {
    return "A bundle ID uses letters, digits, hyphens and dots, like com.yourname.myapp.";
  }
  if (isPlaceholder(v)) {
    return "com.example is a placeholder Apple never registers. Use your own prefix.";
  }
  return undefined;
}

/** The last component for an app named `name`: its letters, digits and hyphens, lowercased. */
export function appComponent(name: string): string {
  const s = name
    .normalize("NFKD")
    .replace(/[^A-Za-z0-9-]/g, "")
    .toLowerCase();
  return s || "app";
}

/** The bundle ID suggested for an app named `name` under `prefix`. */
export function suggestedBundleId(prefix: string, name: string): string {
  return `${prefix.trim()}.${appComponent(name)}`;
}

/** `russet.toml` with the `bundle-id = "<from>"` line set to `to`, or undefined when no such line exists. */
export function replaceBundleId(toml: string, from: string, to: string): string | undefined {
  const escaped = from.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const line = new RegExp(`^(\\s*bundle-id\\s*=\\s*)"${escaped}"`, "m");
  return line.test(toml) ? toml.replace(line, `$1"${to}"`) : undefined;
}

/** The saved organization identifier, asking for it (and saving it) the first time. Undefined: cancelled. */
export async function organizationIdentifier(): Promise<string | undefined> {
  const config = vscode.workspace.getConfiguration();
  const saved = (config.get<string>(ORGANIZATION_SETTING) ?? "").trim();
  if (saved && !prefixProblem(saved)) {
    return saved;
  }
  const value = await vscode.window.showInputBox({
    title: "Organization identifier",
    prompt: "Your reverse-DNS prefix for app bundle IDs. Russet saves it and uses it for every new app.",
    placeHolder: "com.yourname",
    ignoreFocusOut: true,
    validateInput: prefixProblem,
  });
  if (value === undefined) {
    return undefined;
  }
  await config.update(ORGANIZATION_SETTING, value.trim(), vscode.ConfigurationTarget.Global);
  return value.trim();
}

/** Asks for the bundle ID of a new app named `name`, suggesting one under the organization identifier. */
export async function chooseBundleId(name: string): Promise<string | undefined> {
  const prefix = await organizationIdentifier();
  if (!prefix) {
    return undefined;
  }
  return askBundleId(suggestedBundleId(prefix, name));
}

/**
 * Before a run: an app that still has a placeholder identifier gets the user's own, written to its
 * `russet.toml`. False when the user cancels or the file has no matching line (it is then opened).
 */
export async function ensureRegistrable(bundleId: string | undefined, root: string | undefined): Promise<boolean> {
  if (!bundleId || !root || !isPlaceholder(bundleId)) {
    return true;
  }
  const change = "Change Identifier";
  const choice = await vscode.window.showWarningMessage(
    `${bundleId} is a placeholder Apple never registers, so this app cannot be signed with it.`,
    { modal: true },
    change,
  );
  if (choice !== change) {
    return false;
  }
  const prefix = await organizationIdentifier();
  const next = prefix && (await askBundleId(`${prefix}.${bundleId.split(".").pop() ?? "app"}`));
  if (!next) {
    return false;
  }
  const uri = vscode.Uri.joinPath(vscode.Uri.file(root), "russet.toml");
  const text = new TextDecoder().decode(await vscode.workspace.fs.readFile(uri));
  const updated = replaceBundleId(text, bundleId, next);
  if (updated === undefined) {
    await vscode.window.showTextDocument(uri);
    void vscode.window.showWarningMessage(`Set bundle-id to "${next}" in russet.toml, then run again.`);
    return false;
  }
  await vscode.workspace.fs.writeFile(uri, new TextEncoder().encode(updated));
  return true;
}

async function askBundleId(suggestion: string): Promise<string | undefined> {
  const value = await vscode.window.showInputBox({
    title: "Bundle identifier",
    prompt: "Unique to you across Apple. It identifies the app on devices and in the App Store.",
    value: suggestion,
    ignoreFocusOut: true,
    validateInput: bundleIdProblem,
  });
  return value?.trim() || undefined;
}
