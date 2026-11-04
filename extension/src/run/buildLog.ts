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
  started: Map<Stage, number>;
  timings: [Stage, number][];
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
  ) {}

  /** `job.updated`. */
  update(job: JobInfo): void {
    if (!isBuildJob(job.kind)) return;
    let t = this.jobs.get(job.jobId);
    if (!t) {
      t = { kind: job.kind, label: JOB_LABEL[job.kind] ?? job.kind, percent: 0, started: new Map(), timings: [], ended: false };
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
    const parts = t.timings.map(([s, ms]) => `${STAGE_LABEL[s]} ${seconds(ms)}`).join(" · ");
    let summary: string;
    if (succeeded) {
      summary = `✔ ${t.label} succeeded${took}${parts ? ` (${parts})` : ""}`;
    } else {
      const why = job.error?.chain?.[0];
      const verb = job.state === "cancelled" ? "was cancelled" : job.state === "interrupted" ? "was interrupted" : "failed";
      const step = t.stage && t.started.has(t.stage) ? STAGE_LABEL[t.stage] : undefined;
      const where = step ? ` while ${step.charAt(0).toLowerCase()}${step.slice(1)}` : "";
      summary = `✖ ${t.label} ${verb}${took}${where}${why ? `: ${why}` : ""}`;
    }
    this.write(t.kind, summary);
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
          if (!(ev.stream === "info" && STAGE_LOG_LINE.test(ev.line))) this.write(t.kind, ev.line.replace(ANSI, ""));
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
            this.progress(jobId, t, STAGE_LABEL[t.stage], lo + (hi - lo) * Math.min(1, Math.max(0, fraction)));
          }
          break;
        }
        default:
          break;
      }
    }
  }

  private stage(jobId: string, t: Track, stage: Stage, state: StepState, at: number): void {
    const [lo, hi] = BAND[stage];
    if (state === "started") {
      t.stage = stage;
      t.started.set(stage, at);
      this.write(t.kind, `▸ ${STAGE_LABEL[stage]}…`);
      this.progress(jobId, t, STAGE_LABEL[stage], lo);
      return;
    }
    const since = t.started.get(stage);
    const took = since !== undefined ? at - since : undefined;
    if (took !== undefined) t.timings.push([stage, took]);
    const suffix = took !== undefined ? ` (${seconds(took)})` : "";
    if (state === "finished") {
      this.write(t.kind, `✔ ${STAGE_LABEL[stage]}${suffix}`);
      this.progress(jobId, t, STAGE_LABEL[stage], hi);
    } else {
      this.write(t.kind, `✖ ${STAGE_LABEL[stage]} failed${suffix}`);
    }
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
      (kind, line) => {
        this.channel.appendLine(line);
        this._onDidWrite.fire({ kind, line });
      },
      (p) => this._onDidProgress.fire(p),
      (e) => {
        this.revealed.delete(e.job.jobId);
        if (!e.succeeded && reveal() !== "never") this.show();
        this._onDidEnd.fire(e);
      },
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

function reveal(): string {
  return vscode.workspace.getConfiguration("russet").get<string>("build.revealOutput", "onError");
}
