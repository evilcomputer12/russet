// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Bundles the extension host entry point to dist/extension.js (CommonJS, vscode external).

import { build, context } from "esbuild";

const watch = process.argv.includes("--watch");
const production = process.argv.includes("--production");
const tests = process.argv.includes("--tests");

/** @type {import("esbuild").BuildOptions} */
const extension = {
  entryPoints: ["src/extension.ts"],
  bundle: true,
  outfile: "dist/extension.js",
  external: ["vscode"],
  format: "cjs",
  platform: "node",
  target: "node20",
  sourcemap: !production,
  minify: production,
  logLevel: "info",
};

/** The @vscode/test-cli integration suite, bundled to CommonJS. */
const integration = {
  entryPoints: ["test/integration/debug.test.ts"],
  bundle: true,
  outdir: "dist-test/integration",
  external: ["vscode", "mocha"],
  format: "cjs",
  platform: "node",
  target: "node20",
  sourcemap: true,
  logLevel: "info",
};

if (watch) {
  const ctx = await context(extension);
  await ctx.watch();
  console.log("[esbuild] watching…");
} else if (tests) {
  await build(integration);
} else {
  await build(extension);
}
