// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The "Russet Build" output and the progress of builds and runs (EXT-05, ide.md §5.7). The daemon's build,
// run and debug jobs stream their job.events here: the compiler's output and each stage with its time go to
// the output channel, and the stages and progress events make one percentage for the status bar and the
// "Running on iPhone" notification. BuildTracker is the part without VS Code.

import * as vscode from "vscode";
import type { JobEvent, JobInfo, Stage, StepState } from "../generated/protocol";
import { Emitter } from "../util/disposables";
import { windowsPaths } from "../util/docs";
import { explain } from "./explain";

/** The job kinds that build, and how the output names them. */
const JOB_LABEL: Record<string, string> = {
  "build.start": "Build",
  "run.start": "Run",
  "debug.prepare": "Debug",
  "ship.archive": "Archive",
};

/** The jobs that only build: the build stages make all of their percentage. */
const BUILD_ONLY = new Set(["build.start", "ship.archive"]);

export const STAGE_LABEL: Record<Stage, string> = {
  resolve: "Finding the iPhone",
  prepare: "Preparing",
  compile: "Compiling",
  resources: "Copying resources",
  assemble: "Assembling the app",
  lint: "Checking the app",
  package: "Packaging",
  sign: "Signing",
  install: "Installing on the iPhone",
  launch: "Launching",
  attach: "Attaching the debugger",
};

/** Where each stage's own progress lands in a run's percentage. The build stages end at BUILD_END. */
const BAND: Record<Stage, [number, number]> = {
  resolve: [0, 2],
  prepare: [2, 5],
  compile: [5, 64],
  resources: [64, 66],
  assemble: [66, 68],
  lint: [68, 70],
  package: [68, 70],
  sign: [70, 74],
  install: [74, 96],
  launch: [96, 100],
  attach: [96, 100],
};
const BUILD_END = 70;

/** The pipeline's own log line for a stage, which the stage events already show. */
const STAGE_LOG_LINE = /^stage \w+ (started|finished|failed)$/;
/**
 * Tool chatter the concise output leaves out (the job's log file keeps all of it): swift-build's planning steps, its
 * lines for a task or build that failed (they name no error; the compiler's own line above does), and the
 * compiler's invocation.
 */
const CHATTER = [
  /^Building for (debugging|production)\.\.\.$/,
  /^\[(Computing dependencies|Using on-disk description|Provisioning \d+ \/ \d+)\]$/,
  /^\s*builtin-\w+/,
  /^error: .* failed with a nonzero exit code\./,
  /^error: Build failed$/,
];
/** The compiler's line before its whole command line, which the concise output leaves out with it. */
const INVOCATION = "Failed frontend command:";
/** Terminal colours, which an output channel shows as text. */
// eslint-disable-next-line no-control-regex
const ANSI = /\u001b\[[0-?]*[ -/]*[@-~]/g;

export interface BuildProgress {
  jobId: string;
  kind: string;
  /** 0–100, never going back within a job. */
  percent: number;
  /** What runs now, e.g. "Compiling" or "Installing on the iPhone". */
  step: string;
}

export interface BuildEnd {
  job: JobInfo;
  succeeded: boolean;
  /** The one-line summary the output got. */
  summary: string;
}

interface Track {
  kind: string;
  label: string;
  percent: number;
  stage?: Stage;
  /** What the stage at work is called: a run's first resolve finds the iPhone, the build's own checks the project. */
  step?: string;
  started: Map<Stage, { at: number; step: string }>;
  timings: [string, number][];
  resolved: boolean;
  /** The compiler's errors at their place in the code, once each: the summary lists them. */
  errors: { at: string; message: string }[];
  warnings: number;
  /** The next log line is the compiler's command line (after "Failed frontend command:"). */
  skipNext: boolean;
  ended: boolean;
  waiting?: string;
}

export function isBuildJob(kind: string): boolean {
  return kind in JOB_LABEL;
}

export function seconds(ms: number): string {
  return ms >= 60_000 ? `${Math.floor(ms / 60_000)} min ${Math.round((ms % 60_000) / 1000)} s` : `${(ms / 1000).toFixed(1)} s`;
}

function time(ts: string | undefined): number | undefined {
  const t = ts ? Date.parse(ts) : NaN;
  return Number.isNaN(t) ? undefined : t;
}

export class BuildTracker {
  private readonly jobs = new Map<string, Track>();

  constructor(
    private readonly write: (kind: string, line: string) => void,
    private readonly onProgress: (p: BuildProgress) => void,
    private readonly onEnd: (e: BuildEnd) => void,
    /** Whether tool chatter is left out (`russet.build.outputDetail`: concise, the default). */
    private readonly concise: () => boolean = () => true,
  ) {}

  /** `job.updated`. */
  update(job: JobInfo): void {
    if (!isBuildJob(job.kind)) return;
    let t = this.jobs.get(job.jobId);
    if (!t) {
      t = {
        kind: job.kind, label: JOB_LABEL[job.kind] ?? job.kind, percent: 0, started: new Map(), timings: [], resolved: false,
        errors: [], warnings: 0, skipNext: false, ended: false,
      };
      this.jobs.set(job.jobId, t);
      const project = job.project ? job.project.split(/[\\/]/).filter(Boolean).pop() : undefined;
      const at = new Date(time(job.createdAt) ?? Date.now()).toLocaleTimeString();
      this.write(t.kind, "");
      this.write(t.kind, `━━ ${t.label}${project ? ` ${project}` : ""} · ${at} ━━`);
      this.progress(job.jobId, t, "Starting");
    }
    if (job.state === "queued" && job.waitingFor && t.waiting !== job.waitingFor) {
      t.waiting = job.waitingFor;
      this.write(t.kind, `Waiting for ${job.waitingFor}…`);
    }
    if (t.ended || (job.state === "queued" || job.state === "running")) return;
    t.ended = true;
    const succeeded = job.state === "succeeded";
    const took = job.durationMs !== undefined ? ` in ${seconds(job.durationMs)}` : "";
    const parts = t.timings.map(([step, ms]) => `${step} ${seconds(ms)}`).join(" · ");
    let summary: string;
    const warned = t.warnings > 0 ? ` · ${t.warnings} warning${t.warnings === 1 ? "" : "s"}` : "";
    if (succeeded) {
      summary = `✔ ${t.label} succeeded${took}${parts ? ` (${parts})` : ""}${warned}`;
    } else {
      const verb = job.state === "cancelled" ? "was cancelled" : job.state === "interrupted" ? "was interrupted" : "failed";
      const step = t.stage && t.started.has(t.stage) ? t.step : undefined;
      const where = step ? ` while ${step.charAt(0).toLowerCase()}${step.slice(1)}` : "";
      // The compiler's errors, listed below, say it better than the error's own message, which repeats the first.
      const n = t.errors.length;
      const why = n > 0 ? `${n} error${n === 1 ? "" : "s"}` : job.error?.chain?.[0];
      summary = `✖ ${t.label} ${verb}${took}${where}${why ? `: ${why}` : ""}`;
    }
    this.write(t.kind, summary);
    for (const e of t.errors) {
      this.write(t.kind, `  ${e.at}: ${e.message}`);
      // What it means, in plain words, for the common ones (run/explain.ts).
      const meaning = explain(e.message);
      if (meaning) this.write(t.kind, `    ↳ ${meaning}`);
    }
    if (!succeeded && job.error?.hint) this.write(t.kind, `  ${job.error.hint}`);
    if (succeeded) this.progress(job.jobId, t, "Done", 100);
    this.onEnd({ job, succeeded, summary });
    this.jobs.delete(job.jobId);
  }

  /** `job.events` of one job. */
  events(jobId: string, events: readonly JobEvent[]): void {
    const t = this.jobs.get(jobId);
    if (!t) return;
    for (const ev of events) {
      switch (ev.kind) {
        case "log":
          this.log(t, ev.stream, ev.line.replace(ANSI, ""));
          break;
        case "diagnostic":
          if (ev.severity === "warning") t.warnings += 1;
          if (ev.severity === "error" && ev.file) {
            const at = [ev.file, ev.line, ev.column].filter((x) => x !== undefined).join(":");
            if (!t.errors.some((e) => e.at === at && e.message === ev.message)) t.errors.push({ at, message: ev.message });
          }
          break;
        case "warning":
          this.write(t.kind, `warning: ${ev.message}${ev.hint ? ` (${ev.hint})` : ""}`);
          break;
        case "stage":
          this.stage(jobId, t, ev.stage, ev.state, time(ev.ts) ?? Date.now());
          break;
        case "progress": {
          const fraction = ev.done !== undefined && ev.total ? ev.done / ev.total : ev.fraction;
          if (fraction !== undefined && t.stage) {
            const [lo, hi] = BAND[t.stage];
            this.progress(jobId, t, t.step ?? STAGE_LABEL[t.stage], lo + (hi - lo) * Math.min(1, Math.max(0, fraction)));
          }
          break;
        }
        default:
          break;
      }
    }
  }

  private log(t: Track, stream: string, line: string): void {
    if (stream === "info" && STAGE_LOG_LINE.test(line)) return;
    if (this.concise()) {
      if (t.skipNext) {
        t.skipNext = false;
        return;
      }
      if (line === INVOCATION) {
        t.skipNext = true;
        return;
      }
      if (CHATTER.some((re) => re.test(line))) return;
    }
    this.write(t.kind, line);
  }

  private stage(jobId: string, t: Track, stage: Stage, state: StepState, at: number): void {
    const [lo, hi] = BAND[stage];
    if (state === "started") {
      const step = this.stepOf(t, stage);
      t.stage = stage;
      t.step = step;
      t.started.set(stage, { at, step });
      this.write(t.kind, `▸ ${step}…`);
      this.progress(jobId, t, step, lo);
      return;
    }
    const since = t.started.get(stage);
    const step = since?.step ?? STAGE_LABEL[stage];
    const took = since !== undefined ? at - since.at : undefined;
    if (took !== undefined) t.timings.push([step, took]);
    const suffix = took !== undefined ? ` (${seconds(took)})` : "";
    if (state === "finished") {
      this.write(t.kind, `✔ ${step}${suffix}`);
      this.progress(jobId, t, step, hi);
    } else {
      this.write(t.kind, `✖ ${step} failed${suffix}`);
    }
  }

  /** A run resolves twice: the pipeline finds the iPhone, then the build checks the project and its toolchain. */
  private stepOf(t: Track, stage: Stage): string {
    if (stage !== "resolve") return STAGE_LABEL[stage];
    const device = !BUILD_ONLY.has(t.kind) && !t.resolved;
    t.resolved = true;
    return device ? STAGE_LABEL.resolve : "Checking the project";
  }

  private progress(jobId: string, t: Track, step: string, runPercent?: number): void {
    if (runPercent !== undefined) {
      const scaled = BUILD_ONLY.has(t.kind) ? (runPercent * 100) / BUILD_END : runPercent;
      t.percent = Math.max(t.percent, Math.min(100, Math.round(scaled)));
    }
    this.onProgress({ jobId, kind: t.kind, percent: t.percent, step });
  }
}

/** The VS Code side: the output channel, `russet.build.revealOutput`, and the progress for the UI. */
export class BuildLog implements vscode.Disposable {
  readonly channel: vscode.OutputChannel;
  private readonly tracker: BuildTracker;
  private readonly _onDidProgress = new Emitter<BuildProgress>();
  readonly onDidProgress = this._onDidProgress.event;
  private readonly _onDidEnd = new Emitter<BuildEnd>();
  readonly onDidEnd = this._onDidEnd.event;
  /** Each line as written, with its job's kind (the build task's terminal shows its own). */
  private readonly _onDidWrite = new Emitter<{ kind: string; line: string }>();
  readonly onDidWrite = this._onDidWrite.event;
  private readonly revealed = new Set<string>();

  constructor() {
    this.channel = vscode.window.createOutputChannel("Russet Build");
    this.tracker = new BuildTracker(
      (kind, raw) => {
        const line = readable(raw);
        this.channel.appendLine(line);
        this._onDidWrite.fire({ kind, line });
      },
      (p) => this._onDidProgress.fire(p),
      (e) => {
        this.revealed.delete(e.job.jobId);
        if (!e.succeeded && reveal() !== "never") this.show();
        this._onDidEnd.fire(e);
      },
      () => vscode.workspace.getConfiguration("russet").get<string>("build.outputDetail", "concise") !== "full",
    );
  }

  update(job: JobInfo): void {
    this.tracker.update(job);
    if (isBuildJob(job.kind) && job.state === "running" && reveal() === "always" && !this.revealed.has(job.jobId)) {
      this.revealed.add(job.jobId);
      this.show();
    }
  }

  events(jobId: string, events: readonly JobEvent[]): void {
    this.tracker.events(jobId, events);
  }

  show(): void {
    this.channel.show(true);
  }

  dispose(): void {
    this.channel.dispose();
    this._onDidProgress.dispose();
    this._onDidEnd.dispose();
    this._onDidWrite.dispose();
  }
}

/** Past this, a line (a compiler's whole command line) is cut short in the output; the job's log file keeps it. */
const LONGEST_LINE = 600;

/**
 * A line as the output shows it: the backend's Windows-drive paths as Windows paths (C:\…\App.swift:10:13, which the
 * output makes a link), and a line too long to read cut short.
 */
export function readable(line: string, platform: NodeJS.Platform = process.platform): string {
  const text = windowsPaths(line, platform);
  return text.length > LONGEST_LINE
    ? `${text.slice(0, 240)} … (${text.length - 240} more characters, in the build's log file)`
    : text;
}

function reveal(): string {
  return vscode.workspace.getConfiguration("russet").get<string>("build.revealOutput", "onError");
}
