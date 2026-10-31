// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The Windows setup-runner contract (russet-windows docs/IDE-EXPERIENCE-PLAN.md,
// "Setup runner contract"): `Russet.Host.exe --setup status` prints one JSON line,
// `--setup run <id> [--arg key=value]…` prints NDJSON progress. Pure parsing and
// validation only, so the page can be tested against a fake runner.

/** The runner's step ids, in the contract's order. Unknown ids are still shown. */
export const KNOWN_STEP_IDS = [
  "wsl",
  "backend",
  "apple-devices",
  "iphone",
  "xtool",
  "xcode-sdk",
  "vphone",
  "virtual-iphone",
] as const;

export type KnownStepId = (typeof KNOWN_STEP_IDS)[number];
export type SetupStepState = "done" | "needed" | "running" | "failed" | "unavailable";

export interface SetupStepStatus {
  id: string;
  state: SetupStepState;
  title: string;
  detail?: string;
}

export interface SetupStatus {
  version: 1;
  steps: SetupStepStatus[];
}

export type SetupRunEvent =
  | { step: string; event: "progress"; message: string; fraction?: number }
  | { step: string; event: "log"; message: string }
  | { step: string; event: "done" }
  | { step: string; event: "failed"; code: string; message: string; retryable: boolean };

export class SetupProtocolError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SetupProtocolError";
  }
}

const STEP_ID = /^[a-z][a-z0-9-]{0,47}$/;
const ARG_KEY = /^[a-z][a-z0-9-]{0,31}$/;
const STATES: ReadonlySet<string> = new Set(["done", "needed", "running", "failed", "unavailable"]);
const MAX_TEXT = 600;

export function isStepId(value: unknown): value is string {
  return typeof value === "string" && STEP_ID.test(value);
}

/** User-facing text from the runner: no control characters, bounded length. */
export function cleanText(value: unknown, max = MAX_TEXT): string {
  if (typeof value !== "string") {
    return "";
  }
  // eslint-disable-next-line no-control-regex
  const text = value.replace(/[\u0000-\u0008\u000b-\u001f\u007f\u2028\u2029]/g, " ").trim();
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

/** Parse the single status line. Unknown states read as `unavailable`. */
export function parseStatus(line: string): SetupStatus {
  let value: unknown;
  try {
    value = JSON.parse(line);
  } catch {
    throw new SetupProtocolError("The setup runner printed a status that is not JSON.");
  }
  if (!value || typeof value !== "object") {
    throw new SetupProtocolError("The setup runner status is not an object.");
  }
  const v = value as { version?: unknown; steps?: unknown };
  if (v.version !== 1) {
    throw new SetupProtocolError(`The setup runner speaks version ${String(v.version)}; this IDE needs version 1.`);
  }
  if (!Array.isArray(v.steps)) {
    throw new SetupProtocolError("The setup runner status has no steps.");
  }
  const seen = new Set<string>();
  const steps: SetupStepStatus[] = [];
  for (const raw of v.steps) {
    if (!raw || typeof raw !== "object") {
      continue;
    }
    const s = raw as { id?: unknown; state?: unknown; title?: unknown; detail?: unknown };
    if (!isStepId(s.id) || seen.has(s.id)) {
      continue;
    }
    seen.add(s.id);
    const state = typeof s.state === "string" && STATES.has(s.state) ? (s.state as SetupStepState) : "unavailable";
    const detail = cleanText(s.detail);
    steps.push({ id: s.id, state, title: cleanText(s.title, 120) || s.id, ...(detail ? { detail } : {}) });
  }
  return { version: 1, steps };
}

/**
 * Parse one NDJSON line of `--setup run`. Blank lines return undefined; a line for
 * another step or with an unknown event is a protocol error.
 */
export function parseRunEvent(line: string, expectedStep: string): SetupRunEvent | undefined {
  if (line.trim().length === 0) {
    return undefined;
  }
  let value: unknown;
  try {
    value = JSON.parse(line);
  } catch {
    throw new SetupProtocolError("The setup runner printed a line that is not JSON.");
  }
  if (!value || typeof value !== "object") {
    throw new SetupProtocolError("The setup runner printed an event that is not an object.");
  }
  const e = value as Record<string, unknown>;
  if (e.step !== expectedStep) {
    throw new SetupProtocolError(`The setup runner reported an event for another step (${String(e.step)}).`);
  }
  switch (e.event) {
    case "progress": {
      const f = typeof e.fraction === "number" && Number.isFinite(e.fraction)
        ? Math.min(1, Math.max(0, e.fraction))
        : undefined;
      return { step: expectedStep, event: "progress", message: cleanText(e.message), ...(f === undefined ? {} : { fraction: f }) };
    }
    case "log":
      return { step: expectedStep, event: "log", message: cleanText(e.message, 2000) };
    case "done":
      return { step: expectedStep, event: "done" };
    case "failed": {
      const code = typeof e.code === "string" && /^setup\.[a-z0-9.-]{1,80}$/.test(e.code)
        ? e.code
        : `setup.${expectedStep}.failed`;
      return {
        step: expectedStep,
        event: "failed",
        code,
        message: cleanText(e.message) || "The step failed.",
        retryable: e.retryable !== false,
      };
    }
    default:
      throw new SetupProtocolError(`The setup runner printed an unknown event (${String(e.event)}).`);
  }
}

/** The argv after the runner executable for `--setup run <id> [--arg k=v]…`. */
export function runArgv(id: string, args: Readonly<Record<string, string>> = {}): string[] {
  if (!isStepId(id)) {
    throw new SetupProtocolError(`Invalid setup step id: ${id}`);
  }
  const argv = ["--setup", "run", id];
  for (const [key, value] of Object.entries(args)) {
    if (!ARG_KEY.test(key)) {
      throw new SetupProtocolError(`Invalid setup argument name: ${key}`);
    }
    // eslint-disable-next-line no-control-regex
    if (typeof value !== "string" || value.length === 0 || value.length > 1024 || /[\u0000-\u001f"]/.test(value)) {
      throw new SetupProtocolError(`Invalid value for setup argument ${key}.`);
    }
    argv.push("--arg", `${key}=${value}`);
  }
  return argv;
}
