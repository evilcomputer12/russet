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
      log("\u001b[0;36m9 |\u001b[0;0m     WindowGroup {", "stderr"),
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
    // Terminal colours do not reach the output.
    expect(lines).toContain("9 |     WindowGroup {");
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

  it("shows a failed build concisely: the compiler's error with its code, and a summary that lists it", () => {
    // What Russet 0.6.13 printed for a run on the owner's work PC (the paths shortened).
    const diagnostic = (d: object): JobEvent => ({ seq: ++seq, ts: at(0), kind: "diagnostic", severity: "error", ...d }) as JobEvent;
    tracker.update(job("running"));
    tracker.events("job_1", [
      stage("compile", "started", 0),
      log("Building for debugging..."),
      log("[Computing dependencies]"),
      log("[Provisioning 1 / 4]"),
      log("[Using on-disk description]"),
      diagnostic({ message: "SwiftCompile normal arm64 /mnt/c/T/TestApp.swift failed with a nonzero exit code. Command line:     cd /mnt/c/T/.russet/work" }),
      log("error: SwiftCompile normal arm64 /mnt/c/T/TestApp.swift failed with a nonzero exit code. Command line:     cd /mnt/c/T/.russet/work", "stderr"),
      log("    builtin-SwiftPerFileCompile TestApp.swift", "stderr"),
      diagnostic({ message: "cannot find 'ContentView' in scope", file: "/mnt/c/T/TestApp.swift", line: 10, column: 13 }),
      log("/mnt/c/T/TestApp.swift:10:13: error: cannot find 'ContentView' in scope", "stderr"),
      log("10 |             ContentView()", "stderr"),
      log("   |             `- error: cannot find 'ContentView' in scope", "stderr"),
      log("Failed frontend command:", "stderr"),
      log("/usr/lib/swift/usr/bin/swift-frontend -frontend -c /mnt/c/T/ContentView.swift …", "stderr"),
      diagnostic({ message: "Build failed" }),
      log("error: Build failed", "stderr"),
      stage("compile", "failed", 14),
    ]);
    tracker.update(job("failed", {
      durationMs: 24_200,
      error: { russetCode: "build.compileFailed", retryable: false, chain: ["compilation failed: /mnt/c/T/TestApp.swift:10:13: error: cannot find 'ContentView' in scope"], hint: "Fix the errors listed in the build's diagnostics; the full log is /mnt/c/T/.russet/logs/job_1.jsonl." },
    }));
    expect(lines.slice(lines.indexOf("▸ Compiling…"))).toEqual([
      "▸ Compiling…",
      "/mnt/c/T/TestApp.swift:10:13: error: cannot find 'ContentView' in scope",
      "10 |             ContentView()",
      "   |             `- error: cannot find 'ContentView' in scope",
      "✖ Compiling failed (14.0 s)",
      "✖ Run failed in 24.2 s while compiling: 1 error",
      "  /mnt/c/T/TestApp.swift:10:13: cannot find 'ContentView' in scope",
      "    ↳ Nothing named 'ContentView' is visible here. Check the spelling, whether it was renamed or removed, that it is not `private` in another file, or add the missing `import`.",
      "  Fix the errors listed in the build's diagnostics; the full log is /mnt/c/T/.russet/logs/job_1.jsonl.",
    ]);
  });

  it("shows everything the tools print when asked for the full output", () => {
    const full = new BuildTracker((_k, l) => lines.push(l), () => undefined, () => undefined, () => false);
    full.update(job("running"));
    full.events("job_1", [log("[Computing dependencies]"), log("Failed frontend command:"), log("/usr/bin/swift-frontend -c x")]);
    expect(lines.slice(-3)).toEqual(["[Computing dependencies]", "Failed frontend command:", "/usr/bin/swift-frontend -c x"]);
  });

  it("tells a run's iPhone lookup from the build's own check of the project", () => {
    tracker.update(job("running"));
    tracker.events("job_1", [
      stage("resolve", "started", 0), stage("resolve", "finished", 0),
      stage("resolve", "started", 0), stage("resolve", "finished", 8),
    ]);
    expect(lines.filter((l) => l.startsWith("✔"))).toEqual(["✔ Finding the iPhone (0.0 s)", "✔ Checking the project (8.0 s)"]);
    // A build alone has no iPhone to find.
    tracker.update(job("running", { jobId: "job_2", kind: "build.start" }));
    tracker.events("job_2", [stage("resolve", "started", 0)]);
    expect(lines[lines.length - 1]).toBe("▸ Checking the project…");
  });

  it("formats durations", () => {
    expect(seconds(1234)).toBe("1.2 s");
    expect(seconds(125_000)).toBe("2 min 5 s");
  });
});
