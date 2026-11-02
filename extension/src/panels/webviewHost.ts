// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Shared webview HTML scaffolding: CSP, nonce, and the typed postMessage bridge
// contract (ide.md §5.9). Every webview script is bundled to dist/webview/<page>/.

import * as vscode from "vscode";
import * as crypto from "node:crypto";

export function nonce(): string {
  return crypto.randomBytes(16).toString("base64");
}

export interface WebviewPageOptions {
  /** The page folder under dist/webview (e.g. "console", "mirror"). */
  page: string;
  title: string;
  /** Allow ws://127.0.0.1:<port> in connect-src (mirror only); "any" while the port is not known. */
  mirrorPort?: number | "any";
}

/** Build the HTML document for a webview page with a strict per-page CSP. */
export function renderPage(
  webview: vscode.Webview,
  extensionUri: vscode.Uri,
  opts: WebviewPageOptions,
): string {
  const n = nonce();
  const scriptUri = webview.asWebviewUri(
    vscode.Uri.joinPath(extensionUri, "dist", "webview", opts.page, "index.js"),
  );
  const connectSrc =
    opts.mirrorPort === "any" ? "ws://127.0.0.1:*" : opts.mirrorPort ? `ws://127.0.0.1:${opts.mirrorPort}` : "'none'";
  const csp = [
    "default-src 'none'",
    `img-src ${webview.cspSource} data: blob:`,
    "media-src blob:",
    `style-src ${webview.cspSource} 'unsafe-inline'`,
    `font-src ${webview.cspSource}`,
    `script-src 'nonce-${n}'`,
    "worker-src blob:",
    `connect-src ${connectSrc}`,
  ].join("; ");

  return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta http-equiv="Content-Security-Policy" content="${csp}" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>${escapeHtml(opts.title)}</title>
</head>
<body>
  <div id="root"></div>
  <script nonce="${n}" type="module" src="${scriptUri}"></script>
</body>
</html>`;
}

export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

export function localResourceRoots(extensionUri: vscode.Uri): vscode.Uri[] {
  return [vscode.Uri.joinPath(extensionUri, "dist", "webview")];
}
