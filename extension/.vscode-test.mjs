// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// @vscode/test-cli configuration. The integration suite runs against a self-contained
// fake russetd (test/integration/fake-daemon.ts) because W15's QA-15 mock-daemon
// backend is not merged yet; see the card report. The extension finds the fake
// daemon's socket through RUSSET_HOME (ARCHITECTURE §7.1).

import { defineConfig } from "@vscode/test-cli";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { mkdtempSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const russetHome = mkdtempSync(resolve(tmpdir(), "russet-ext-"));
mkdirSync(resolve(russetHome, "runtime"), { recursive: true });

export default defineConfig({
  files: "dist-test/integration/**/*.test.js",
  version: process.env.RUSSET_VSCODE_VERSION ?? "stable",
  // The card pins VS Code 1.141 and VSCodium 1.135; `fromPath` uses the host install
  // (avoids a download at AMBER). Set RUSSET_VSCODE_PATH to override.
  ...(process.env.RUSSET_VSCODE_PATH ? { fromPath: process.env.RUSSET_VSCODE_PATH } : {}),
  workspaceFolder: resolve(here, "test/fixtures/HelloProject"),
  launchArgs: ["--disable-extensions", "--disable-gpu"],
  env: {
    RUSSET_HOME: russetHome,
    RUSSET_DEVICE_BACKEND: "none",
    RUSSET_SECRETS_BACKEND: "memory",
  },
  mocha: {
    ui: "bdd",
    timeout: 60000,
  },
});
