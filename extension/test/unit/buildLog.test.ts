// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, it, expect, beforeEach } from "vitest";
import { BuildTracker, seconds, type BuildEnd, type BuildProgress } from "../../src/run/buildLog";
import type { JobEvent, JobInfo } from "../../src/generated/protocol";

const job = (state: JobInfo["state"], extra: Partial<JobInfo> = {}): JobInfo => ({
  jobId: "job_1",
  kind: "run.start",
  title: "run.start",
  state,
  background: false,
  createdAt: "2026-11-04T09:00:00.000Z",
  project: "C:\\Users\\someone\\Test",
  ...extra,
});

let seq = 0;
const at = (s: number): string => new Date(Date.UTC(2026, 10, 4, 9, 0, s)).toISOString();
const stage = (name: string, state: string, s: number): JobEvent =>
  ({ seq: ++seq, ts: at(s), kind: "stage", stage: name, state }) as JobEvent;
const log = (line: string, stream = "stdout"): JobEvent => ({ seq: ++seq, ts: at(0), kind: "log", stream, line }) as JobEvent;
const progress = (p: { done?: number; total?: number; fraction?: number }): JobEvent =>
  ({ seq: ++seq, ts: at(0), kind: "progress", ...p }) as JobEvent;

describe("the Russet Build output", () => {
  let lines: string[];
  let percents: BuildProgress[];
  let ends: BuildEnd[];
  let tracker: BuildTracker;

  beforeEach(() => {
    lines = [];
    percents = [];
    ends = [];
    tracker = new BuildTracker((_kind, l) => lines.push(l), (p) => percents.push(p), (e) => ends.push(e));
  });

  it("shows a run's output, each stage with its time, and one rising percentage", () => {
    tracker.update(job("running"));
    tracker.events("job_1", [
      stage("resolve", "started", 0),
      stage("resolve", "finished", 1),
      log("stage resolve finished", "info"),
      stage("compile", "started", 1),
      log("[1/4] Compiling Test ContentView.swift"),
      progress({ done: 1, total: 4 }),
      progress({ done: 4, total: 4 }),
      stage("compile", "finished", 31),
      stage("sign", "started", 31),
      stage("sign", "finished", 32),
      stage("install", "started", 32),
      progress({ fraction: 0.5 }),
      progress({ fraction: 0.25 }),
      stage("install", "finished", 72),
      stage("launch", "started", 72),
      stage("launch", "finished", 74),
    ]);
    tracker.update(job("succeeded", { durationMs: 74_000 }));

    expect(lines).toContain("[1/4] Compiling Test ContentView.swift");
    expect(lines).toContain("✔ Compiling (30.0 s)");
    expect(lines).toContain("✔ Installing on the iPhone (40.0 s)");
    // The pipeline's own "stage … finished" line is not repeated.
    expect(lines.some((l) => l.startsWith("stage "))).toBe(false);
    expect(lines.find((l) => l.startsWith("━━"))).toMatch(/^━━ Run Test · /);
    expect(ends[0]?.succeeded).toBe(true);
    expect(ends[0]?.summary).toBe(
      "✔ Run succeeded in 1 min 14 s (Finding the iPhone 1.0 s · Compiling 30.0 s · Signing 1.0 s · Installing on the iPhone 40.0 s · Launching 2.0 s)",
    );

    const p = percents.map((x) => x.percent);
    expect(p.every((v, i) => i === 0 || v >= p[i - 1])).toBe(true);
    expect(p[p.length - 1]).toBe(100);
    const installing = percents.find((x) => x.step === "Installing on the iPhone" && x.percent > 74);
    expect(installing?.percent).toBe(85);
  });

  it("names the stage and the reason when a run fails, and its hint", () => {
    tracker.update(job("running"));
    tracker.events("job_1", [stage("install", "started", 0), stage("install", "failed", 5)]);
    tracker.update(job("failed", { durationMs: 5000, error: { russetCode: "device.x", retryable: false, chain: ["the iPhone is locked"], hint: "Unlock it and run again." } }));
    expect(lines).toContain("✖ Installing on the iPhone failed (5.0 s)");
    expect(ends[0]?.summary).toBe("✖ Run failed in 5.0 s while installing on the iPhone: the iPhone is locked");
    expect(lines[lines.length - 1]).toBe("  Unlock it and run again.");
  });

  it("gives a build all of its percentage, and ignores jobs that do not build", () => {
    tracker.update(job("running", { jobId: "job_2", kind: "build.start" }));
    tracker.events("job_2", [stage("compile", "started", 0), progress({ done: 1, total: 2 })]);
    expect(percents[percents.length - 1]?.percent).toBe(49);
    tracker.update(job("running", { jobId: "job_3", kind: "toolchain.install" }));
    tracker.events("job_3", [log("unrelated")]);
    expect(lines).not.toContain("unrelated");
  });

  it("formats durations", () => {
    expect(seconds(1234)).toBe("1.2 s");
    expect(seconds(125_000)).toBe("2 min 5 s");
  });
});
