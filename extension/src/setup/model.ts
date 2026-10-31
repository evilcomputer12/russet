// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The guided-setup state: the runner's steps plus the daemon-backed Apple account
// step, the running step's progress and log tail, the Xcode archive and the
// virtual-iPhone choice. UI-independent, so the page and the tests share it; the
// walkthrough completion keys are derived from it.

import type { SetupRunEvent, SetupStatus, SetupStepState } from "./contract";
import type { SetupRunOutcome } from "./runner";
import { cleanText, isStepId } from "./contract";
import { DEFAULT_VIRTUAL, isValidChoice, type VirtualIphoneChoice } from "./virtualCatalog";
import { isXipPath, type XipCandidate } from "./xcode";

export interface SetupRunnerLike {
  status(): Promise<SetupStatus>;
  run(
    id: string,
    args: Readonly<Record<string, string>>,
    onEvent: (event: SetupRunEvent) => void,
    signal?: AbortSignal,
  ): Promise<SetupRunOutcome>;
}

export interface DaemonProbe {
  connected(): boolean;
  /** Undefined when the daemon cannot answer. */
  accounts(): Promise<{ count: number; label?: string } | undefined>;
  toolchain(): Promise<{ swiftOk: boolean; sdkCount: number } | undefined>;
  /**
   * Physical iPhones the daemon sees: `ready` names one that is ready (by its
   * model name, never the user's device name), `waiting` one that needs trust.
   */
  devices?(): { ready?: string; waiting?: string };
}

export interface KeyValueStore {
  get<T>(key: string): T | undefined;
  update(key: string, value: unknown): Thenable<void> | Promise<void>;
}

export type StepAction = "run" | "retry" | "repair" | "cancel" | "recheck" | "signIn" | "addKey" | "pickDevice";

export interface SetupViewStep {
  id: string;
  title: string;
  detail?: string;
  state: SetupStepState | "unknown";
  source: "runner" | "daemon";
  progress?: { message: string; fraction?: number };
  log: string[];
  error?: { code: string; message: string; retryable: boolean };
  actions: StepAction[];
  /** Why the primary action is disabled, if it is. */
  blockedReason?: string;
}

export interface SetupSnapshot {
  runnerAvailable: boolean;
  runnerError?: string;
  loading: boolean;
  daemonConnected: boolean;
  steps: SetupViewStep[];
  doneCount: number;
  totalCount: number;
  runningStep?: string;
  runningAll: boolean;
  xcode: {
    candidates: Array<{ path: string; fileName: string; version?: string; sizeBytes: number }>;
    selected?: string;
  };
  virtual: VirtualIphoneChoice;
  account: { label?: string; signInEnabled: boolean };
}

export type ContextKeys = Record<string, boolean>;

const LOG_TAIL = 200;
const ACCOUNT_STEP = "account";
const KEY_VIRTUAL = "russet.setup.virtualChoice";
const KEY_XIP = "russet.setup.xipPath";

interface StepRuntime {
  progress?: { message: string; fraction?: number };
  log: string[];
  error?: { code: string; message: string; retryable: boolean };
}

export interface SetupModelDeps {
  runner?: SetupRunnerLike;
  daemon: DaemonProbe;
  findXips: () => Promise<XipCandidate[]>;
  store: KeyValueStore;
  signInEnabled: () => boolean;
}

export class SetupModel {
  private status?: SetupStatus;
  private runnerError?: string;
  private loading = false;
  private accountInfo?: { count: number; label?: string };
  private toolchainInfo?: { swiftOk: boolean; sdkCount: number };
  private candidates: XipCandidate[] = [];
  private readonly runtime = new Map<string, StepRuntime>();
  private running?: { id: string; abort: AbortController };
  private runningAll = false;
  private stopAll = false;
  private readonly listeners = new Set<() => void>();

  constructor(private readonly deps: SetupModelDeps) {}

  onDidChange(listener: () => void): { dispose(): void } {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  private changed(): void {
    for (const l of [...this.listeners]) {
      l();
    }
  }

  /** Re-render after a change outside the model (devices, settings). */
  notify(): void {
    this.changed();
  }

  get runnerAvailable(): boolean {
    return !!this.deps.runner;
  }

  get virtualChoice(): VirtualIphoneChoice {
    const stored = this.deps.store.get<VirtualIphoneChoice>(KEY_VIRTUAL);
    return isValidChoice(stored) ? stored : { ...DEFAULT_VIRTUAL };
  }

  get selectedXip(): string | undefined {
    const stored = this.deps.store.get<string>(KEY_XIP);
    if (stored && this.candidates.some((c) => c.path === stored)) {
      return stored;
    }
    if (stored && isXipPath(stored) && this.deps.store.get<boolean>(`${KEY_XIP}.browsed`)) {
      return stored;
    }
    return this.candidates[0]?.path;
  }

  stepState(id: string): SetupStepState | undefined {
    return this.status?.steps.find((s) => s.id === id)?.state;
  }

  /** Re-read the runner's status, the daemon state and the Downloads folder. */
  async refresh(): Promise<void> {
    this.loading = true;
    this.changed();
    const runner = this.deps.runner;
    const [status, accounts, toolchain, xips] = await Promise.all([
      runner
        ? runner.status().then(
            (s) => ({ ok: true as const, s }),
            (err: unknown) => ({ ok: false as const, err }),
          )
        : Promise.resolve(undefined),
      this.deps.daemon.accounts().catch(() => undefined),
      this.deps.daemon.toolchain().catch(() => undefined),
      this.deps.findXips().catch(() => [] as XipCandidate[]),
    ]);
    if (status?.ok) {
      this.status = this.mergeRunning(status.s);
      this.runnerError = undefined;
    } else if (status) {
      this.runnerError = cleanText(status.err instanceof Error ? status.err.message : String(status.err));
    }
    this.accountInfo = accounts;
    this.toolchainInfo = toolchain;
    this.candidates = xips;
    this.loading = false;
    this.changed();
  }

  /** Keep our own running step marked running even if the runner reports otherwise. */
  private mergeRunning(status: SetupStatus): SetupStatus {
    if (!this.running) {
      return status;
    }
    const id = this.running.id;
    return { ...status, steps: status.steps.map((s) => (s.id === id ? { ...s, state: "running" } : s)) };
  }

  async setVirtualChoice(choice: VirtualIphoneChoice): Promise<void> {
    if (!isValidChoice(choice)) {
      return;
    }
    await this.deps.store.update(KEY_VIRTUAL, { device: choice.device, ios: choice.ios });
    this.changed();
  }

  /** Select a detected archive (only paths the page itself found are accepted). */
  async selectXip(p: string): Promise<void> {
    if (!this.candidates.some((c) => c.path === p)) {
      return;
    }
    await this.deps.store.update(KEY_XIP, p);
    await this.deps.store.update(`${KEY_XIP}.browsed`, false);
    this.changed();
  }

  /** Select an archive the user picked in the Browse dialog. */
  async browseXip(p: string): Promise<boolean> {
    if (!isXipPath(p)) {
      return false;
    }
    await this.deps.store.update(KEY_XIP, p);
    await this.deps.store.update(`${KEY_XIP}.browsed`, true);
    this.changed();
    return true;
  }

  private argsFor(id: string, repair: boolean): Record<string, string> | { blocked: string } {
    const args: Record<string, string> = {};
    if (id === "xcode-sdk") {
      const xip = this.selectedXip;
      if (!xip) {
        return { blocked: "Choose the Xcode archive (.xip) you downloaded from Apple." };
      }
      args.xip = xip;
    } else if (id === "virtual-iphone") {
      const v = this.virtualChoice;
      args.device = v.device;
      args.ios = v.ios;
    }
    if (repair) {
      args.repair = "1";
    }
    return args;
  }

  /** Run (or retry, or repair) one runner step. Only one step runs at a time. */
  async runStep(id: string, mode: "run" | "retry" | "repair" = "run"): Promise<SetupRunOutcome | undefined> {
    const runner = this.deps.runner;
    if (!runner || this.running || !isStepId(id) || !this.status?.steps.some((s) => s.id === id)) {
      return undefined;
    }
    const args = this.argsFor(id, mode === "repair");
    if ("blocked" in args) {
      this.runtime.set(id, { log: [], error: { code: `setup.${id}.input`, message: args.blocked, retryable: true } });
      this.changed();
      return { kind: "failed", code: `setup.${id}.input`, message: args.blocked, retryable: true };
    }
    const abort = new AbortController();
    this.running = { id, abort };
    const rt: StepRuntime = { log: [], progress: { message: "Starting…" } };
    this.runtime.set(id, rt);
    this.setLocalState(id, "running");
    this.changed();

    const outcome = await runner.run(id, args, (event) => this.onEvent(rt, event), abort.signal);
    this.running = undefined;
    rt.progress = undefined;
    if (outcome.kind === "failed") {
      rt.error = { code: outcome.code, message: outcome.message, retryable: outcome.retryable };
      this.setLocalState(id, "failed");
    } else if (outcome.kind === "cancelled") {
      rt.log.push("Cancelled.");
      this.setLocalState(id, "needed");
    } else {
      this.setLocalState(id, "done");
    }
    this.changed();
    await this.refresh();
    return outcome;
  }

  private onEvent(rt: StepRuntime, event: SetupRunEvent): void {
    switch (event.event) {
      case "progress":
        // A new phase goes to the log once; repeated ticks only move the bar.
        if (event.message && event.message !== rt.progress?.message) {
          pushTail(rt.log, event.message);
        }
        rt.progress = { message: event.message, ...(event.fraction === undefined ? {} : { fraction: event.fraction }) };
        break;
      case "log":
        pushTail(rt.log, event.message);
        break;
      default:
        break;
    }
    this.changed();
  }

  private setLocalState(id: string, state: SetupStepState): void {
    if (!this.status) {
      return;
    }
    this.status = {
      ...this.status,
      steps: this.status.steps.map((s) => (s.id === id ? { ...s, state } : s)),
    };
  }

  cancel(): void {
    this.stopAll = true;
    this.running?.abort.abort();
  }

  /**
   * Run every step that still needs work, in order, stopping at the first
   * failure or cancel. Steps waiting for input (no Xcode archive) are skipped.
   */
  async runAll(): Promise<void> {
    if (this.running || this.runningAll || !this.deps.runner) {
      return;
    }
    this.runningAll = true;
    this.stopAll = false;
    this.changed();
    try {
      await this.refresh();
      for (const step of [...(this.status?.steps ?? [])]) {
        if (this.stopAll) {
          break;
        }
        const state = this.stepState(step.id);
        if (state !== "needed" && state !== "failed") {
          continue;
        }
        if ("blocked" in this.argsFor(step.id, false)) {
          continue;
        }
        const outcome = await this.runStep(step.id, state === "failed" ? "retry" : "run");
        if (!outcome || outcome.kind !== "done") {
          break;
        }
      }
    } finally {
      this.runningAll = false;
      this.changed();
    }
  }

  snapshot(): SetupSnapshot {
    const steps: SetupViewStep[] = [];
    const busy = !!this.running;
    for (const raw of this.status?.steps ?? []) {
      const s = this.withDeviceState(raw);
      const rt = this.runtime.get(s.id);
      const isMine = this.running?.id === s.id;
      const actions: StepAction[] = [];
      let blockedReason: string | undefined;
      if (s.id === "iphone" && !isMine) {
        // The runner hands iPhone readiness (pairing, Developer Mode, DDI) to the
        // device picker; the page links there instead of running a step.
        actions.push("pickDevice");
        if (s.state !== "done") {
          actions.push("recheck");
        }
      } else if (isMine) {
        actions.push("cancel");
      } else if (s.state === "needed") {
        actions.push(rt?.error ? "retry" : "run");
      } else if (s.state === "failed") {
        actions.push(rt?.error?.retryable === false ? "repair" : "retry");
      } else if (s.state === "done") {
        actions.push("repair");
      } else if (s.state === "unavailable") {
        actions.push("recheck");
      }
      if (!isMine && actions[0] !== "recheck" && actions[0] !== "pickDevice") {
        const args = this.argsFor(s.id, false);
        if ("blocked" in args) {
          blockedReason = args.blocked;
        } else if (busy || this.runningAll) {
          blockedReason = "Another step is running.";
        }
      }
      steps.push({
        id: s.id,
        title: s.title,
        ...(s.detail ? { detail: s.detail } : {}),
        state: s.state,
        source: "runner",
        ...(rt?.progress ? { progress: rt.progress } : {}),
        log: rt ? rt.log.slice(-50) : [],
        ...(rt?.error && !isMine && s.state !== "done" ? { error: rt.error } : {}),
        actions,
        ...(blockedReason ? { blockedReason } : {}),
      });
    }

    // The Apple account step comes from the daemon, not the runner; it goes
    // before the Xcode step (Russet will download Xcode after sign-in).
    const account = this.accountStep();
    const xcodeIndex = steps.findIndex((s) => s.id === "xcode-sdk");
    steps.splice(xcodeIndex >= 0 ? xcodeIndex : steps.length, 0, account);
    if (!this.deps.runner) {
      steps.unshift(this.toolchainStep());
    }

    const doneCount = steps.filter((s) => s.state === "done").length;
    const selected = this.selectedXip;
    return {
      runnerAvailable: this.runnerAvailable,
      ...(this.runnerError ? { runnerError: this.runnerError } : {}),
      loading: this.loading,
      daemonConnected: this.deps.daemon.connected(),
      steps,
      doneCount,
      totalCount: steps.length,
      ...(this.running ? { runningStep: this.running.id } : {}),
      runningAll: this.runningAll,
      xcode: {
        candidates: this.candidates.map((c) => ({
          path: c.path,
          fileName: c.fileName,
          ...(c.version ? { version: c.version } : {}),
          sizeBytes: c.sizeBytes,
        })),
        ...(selected ? { selected } : {}),
      },
      virtual: this.virtualChoice,
      account: {
        ...(this.accountInfo?.label ? { label: this.accountInfo.label } : {}),
        signInEnabled: this.deps.signInEnabled(),
      },
    };
  }

  /** A ready iPhone in the daemon's device list completes the `iphone` step. */
  private withDeviceState(step: SetupStatus["steps"][number]): SetupStatus["steps"][number] {
    if (step.id !== "iphone" || step.state === "done" || step.state === "running") {
      return step;
    }
    const devices = this.deps.daemon.devices?.();
    if (devices?.ready) {
      return { ...step, state: "done", detail: `Ready: ${devices.ready}.` };
    }
    if (devices?.waiting) {
      return { ...step, state: "needed", detail: `${devices.waiting}. Open the device picker to finish pairing.` };
    }
    return step;
  }

  private accountStep(): SetupViewStep {
    const info = this.accountInfo;
    const connected = this.deps.daemon.connected();
    const actions: StepAction[] = this.deps.signInEnabled() ? ["signIn", "addKey"] : ["addKey"];
    if (info && info.count > 0) {
      return {
        id: ACCOUNT_STEP,
        title: "Sign in to your Apple developer account",
        detail: info.label ? `Signed in: ${info.label}.` : "An account is set up.",
        state: "done",
        source: "daemon",
        log: [],
        actions,
      };
    }
    return {
      id: ACCOUNT_STEP,
      title: "Sign in to your Apple developer account",
      detail: connected
        ? "Sign in with your Apple ID (free or paid) so Russet can sign your apps, or add an App Store Connect API key."
        : "The Russet engine is not running yet. Finish the steps above, then sign in.",
      state: connected ? "needed" : "unavailable",
      source: "daemon",
      log: [],
      actions: connected ? actions : [],
    };
  }

  private toolchainStep(): SetupViewStep {
    const t = this.toolchainInfo;
    return {
      id: "toolchain",
      title: "Swift toolchain and iOS SDK",
      detail: !t
        ? "The Russet engine is not running."
        : t.swiftOk
          ? `Swift is ready; ${t.sdkCount} iOS SDK${t.sdkCount === 1 ? "" : "s"} imported.`
          : "Install the matching Swift toolchain (run the doctor for details).",
      state: !t ? "unknown" : t.swiftOk && t.sdkCount > 0 ? "done" : "needed",
      source: "daemon",
      log: [],
      actions: [],
    };
  }

  /** The walkthrough completion keys (ide.md §5.8), from real state only. */
  contextKeys(): ContextKeys {
    const runnerDone = (id: string): boolean => this.stepState(id) === "done";
    const runnerSteps = (this.status?.steps ?? []).map((s) => this.withDeviceState(s));
    return {
      "russet.windowsSetup": this.runnerAvailable,
      "russet.setupOk": this.runnerAvailable && runnerSteps.length > 0 && runnerSteps.every((s) => s.state === "done"),
      "russet.toolchainOk": !!this.toolchainInfo?.swiftOk || (runnerDone("backend") && runnerDone("xtool")),
      "russet.sdkOk": (this.toolchainInfo?.sdkCount ?? 0) > 0 || runnerDone("xcode-sdk"),
      "russet.accountOk": (this.accountInfo?.count ?? 0) > 0,
      "russet.virtualIphoneOk": runnerDone("virtual-iphone"),
    };
  }
}

function pushTail(lines: string[], line: string): void {
  lines.push(line);
  if (lines.length > LOG_TAIL) {
    lines.splice(0, lines.length - LOG_TAIL);
  }
}
