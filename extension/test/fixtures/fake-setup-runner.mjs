// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// A fake `Russet.Host.exe --setup …` that speaks the setup-runner contract
// (russet-windows docs/IDE-EXPERIENCE-PLAN.md) for unit tests, the installed-
// editor smoke run and screenshots. Never touches WSL or the network.
//
//   FAKE_SETUP_STATE   JSON file with {"<step>": "<state>"} overrides; runs update it.
//   FAKE_SETUP_FAIL    comma-separated step ids whose run fails (retryable).
//   FAKE_SETUP_DELAY   milliseconds between progress events (default 15).

/* global setTimeout */
import * as fs from "node:fs";

const STEPS = [
  ["wsl", "Windows Subsystem for Linux", "done", "Installed."],
  ["backend", "Russet backend", "done", "Ready."],
  ["apple-devices", "Apple Devices (USB)", "done", "Apple Devices is running."],
  ["iphone", "iPhone", "unavailable", "Checked in the device picker once the backend is ready."],
  ["xtool", "xtool", "done", "Installed."],
  ["xcode-sdk", "iOS SDK from Xcode", "needed", "Choose an Xcode .xip to build the iOS SDK."],
  ["vphone", "Virtual iPhone engine", "needed", "Not built yet (about an hour)."],
  ["virtual-iphone", "Virtual iPhone", "unavailable", "Create a virtual iPhone (several hours on first boot)."],
];

const stateFile = process.env.FAKE_SETUP_STATE;
const fail = new Set((process.env.FAKE_SETUP_FAIL || "").split(",").filter(Boolean));
const delay = Number(process.env.FAKE_SETUP_DELAY || 15);

function overrides() {
  if (!stateFile) return {};
  try {
    return JSON.parse(fs.readFileSync(stateFile, "utf8"));
  } catch {
    return {};
  }
}

function save(id, state) {
  if (!stateFile) return;
  const o = overrides();
  o[id] = state;
  if (id === "vphone" && state === "done" && !o["virtual-iphone"]) o["virtual-iphone"] = "needed";
  fs.writeFileSync(stateFile, JSON.stringify(o));
}

function status() {
  const o = overrides();
  return STEPS.map(([id, title, state, detail]) => ({ id, state: o[id] || state, title, detail }));
}

const emit = (obj) => process.stdout.write(JSON.stringify(obj) + "\n");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function run(id, args) {
  const step = status().find((s) => s.id === id);
  if (!step) {
    emit({ step: id, event: "failed", code: `setup.${id}.unknown`, message: "Unknown step.", retryable: false });
    process.exit(1);
  }
  if (step.state === "done" && !args.repair) {
    emit({ step: id, event: "done" });
    process.exit(0);
  }
  if (id === "xcode-sdk" && !/\.xip$/i.test(args.xip || "")) {
    emit({ step: id, event: "failed", code: "setup.xcode-sdk.bad-args", message: "Pass --arg xip=<path to Xcode .xip>.", retryable: false });
    process.exit(1);
  }
  const phases = {
    "xcode-sdk": ["Checking the archive signature", "Expanding Xcode (this takes a while)", "Extracting the iOS SDK", "Building the Swift SDK bundle"],
    vphone: ["Fetching vphone-linux", "Building QEMU", "Building the guest tools"],
    "virtual-iphone": ["Preparing firmware", "Restoring iOS", "First boot"],
  }[id] || ["Checking", "Installing", "Verifying"];
  const total = phases.length * 4;
  let n = 0;
  for (const phase of phases) {
    emit({ step: id, event: "log", message: `== ${phase}` });
    for (let i = 0; i < 4; i++) {
      n++;
      emit({ step: id, event: "progress", message: phase, fraction: n / total });
      if (i === 1) emit({ step: id, event: "log", message: `${phase}: ${n}/${total}` });
      await sleep(delay);
      if (fail.has(id) && n === Math.floor(total / 2)) {
        save(id, "failed");
        emit({ step: id, event: "failed", code: `setup.${id}.backend-error`, message: "The backend reported an error while building.", retryable: true });
        process.exit(1);
      }
    }
  }
  save(id, "done");
  emit({ step: id, event: "done" });
  process.exit(0);
}

const argv = process.argv.slice(2);
if (argv[0] !== "--setup") {
  process.stderr.write("usage: --setup status | --setup run <id> [--arg k=v]…\n");
  process.exit(2);
}
if (argv[1] === "status") {
  emit({ version: 1, steps: status() });
} else if (argv[1] === "run" && argv[2]) {
  const args = {};
  for (let i = 3; i < argv.length; i += 2) {
    if (argv[i] === "--arg" && argv[i + 1]) {
      const eq = argv[i + 1].indexOf("=");
      args[argv[i + 1].slice(0, eq)] = argv[i + 1].slice(eq + 1);
    }
  }
  run(argv[2], args);
} else {
  process.exit(2);
}
