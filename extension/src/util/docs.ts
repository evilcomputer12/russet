// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Russet's documentation ships in the extension (docs/) and opens in the Markdown preview: it works
// offline and always matches the installed Russet. An error's `docs` (ARCHITECTURE §13) names its
// section of the error reference, `docs/reference/errors.md#build.compileFailed`, which is
// docs/errors.md's heading of that name here.

import * as vscode from "vscode";

let root: vscode.Uri | undefined;

/** Called once at activation with the extension's folder. */
export function setDocsRoot(extensionUri: vscode.Uri): void {
  root = vscode.Uri.joinPath(extensionUri, "docs");
}

/** The Markdown preview's anchor for a heading: lower case, punctuation dropped, spaces as dashes. */
export function slug(heading: string): string {
  return heading
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s_-]/gu, "")
    .replace(/\s+/g, "-");
}

/** The bundled page and heading for an error's `docs` value; undefined for a web address. */
export function errorPage(docs: string): { page: string; heading?: string } | undefined {
  if (/^https?:\/\//.test(docs)) {
    return undefined;
  }
  const heading = docs.split("#")[1];
  return { page: "errors.md", ...(heading ? { heading } : {}) };
}

/** Opens a page of the documentation (its contents by default), at `heading` when given. */
export async function openDocs(page = "README.md", heading?: string): Promise<void> {
  if (!root) {
    return;
  }
  const uri = vscode.Uri.joinPath(root, page);
  await vscode.commands.executeCommand("markdown.showPreview", heading ? uri.with({ fragment: slug(heading) }) : uri);
}

/** Opens an error's `docs`: its section of the bundled error reference, or a web page. */
export async function openErrorDocs(docs: string): Promise<void> {
  const local = errorPage(docs);
  if (local) {
    await openDocs(local.page, local.heading);
  } else {
    await vscode.env.openExternal(vscode.Uri.parse(docs));
  }
}

/**
 * Text from the WSL backend with its Windows-drive paths as Windows shows them (/mnt/c/Users/x → C:\Users\x),
 * so a Windows user can open them (an output channel makes `C:\…\File.swift:10:13` a link). Other Linux paths stay.
 */
export function windowsPaths(text: string, platform: NodeJS.Platform = process.platform): string {
  if (platform !== "win32") {
    return text;
  }
  return text.replace(/\/mnt\/([a-z])((?:\/[^\s/:'"`)\]]+)+)/g, (_m, drive: string, rest: string) =>
    `${drive.toUpperCase()}:${rest.replace(/\//g, "\\")}`,
  );
}
