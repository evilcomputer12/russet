// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// vitest runs the pure-logic unit tests (no Electron, no `vscode` module).

import { defineConfig } from "vitest/config";
import { resolve } from "node:path";

export default defineConfig({
  resolve: {
    alias: {
      vscode: resolve(__dirname, "test/unit/vscode-mock.ts"),
    },
  },
  test: {
    include: ["test/unit/**/*.test.ts"],
    environment: "node",
    reporters: "default",
  },
});
