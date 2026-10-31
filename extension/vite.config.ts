// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Builds the webview bundles (Console, iPhone mirror placeholder) into dist/webview/.
// The webview platform is kept dependency-light for Tier 0 (vanilla TS); the full
// React 19 + @vscode-elements platform (ide.md §5.9) lands with the feature panels.

import { defineConfig } from "vite";
import { resolve } from "node:path";

export default defineConfig({
  root: resolve(__dirname, "webview"),
  base: "./",
  build: {
    outDir: resolve(__dirname, "dist/webview"),
    emptyOutDir: true,
    target: "es2022",
    sourcemap: true,
    rollupOptions: {
      input: {
        console: resolve(__dirname, "webview/console/index.ts"),
        mirror: resolve(__dirname, "webview/mirror/index.ts"),
      },
      output: {
        entryFileNames: "[name]/index.js",
        chunkFileNames: "shared/[name].js",
        assetFileNames: "[name]/[name][extname]",
      },
    },
  },
});
