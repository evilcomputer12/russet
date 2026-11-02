// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Keeps Russet's managed components on upstream's latest commit (the setup
// runner's `--setup updates`, russet-windows docs/IDE-EXPERIENCE-PLAN.md). Every
// `russet.updates.checkIntervalSeconds` (30 s by default; 0 turns it off) the
// watcher asks the runner whether upstream moved; when it did, it runs
// `<component>-update`, which builds the new commit beside the build in use and
// switches to it once no virtual iPhone runs. A failed build leaves the build in
// use as it was and is reported once per upstream commit, with Retry; a finished
// switch is reported with Roll Back.

import type { ComponentUpdate, SetupRunEvent } from "./contract";
import type { SetupRunOutcome } from "./runner";

export const DEFAULT_CHECK_INTERVAL_SECONDS = 30;
/** Shorter intervals are raised to this: one `git ls-remote` per component per check. */
export const MIN_CHECK_INTERVAL_SECONDS = 10;

/** The runner calls the watcher makes (SetupRunner's). */
export interface UpdatesRunner {
  updates(): Promise<ComponentUpdate[]>;
  run(id: string, args: Readonly<Record<string, string>>, onEvent: (e: SetupRunEvent) => void): Promise<SetupRunOutcome>;
}

/** What the watcher tells the user; each returns the action picked, if any. */
export interface UpdatesUi {
  info(message: string, ...actions: string[]): Promise<string | undefined>;
  warn(message: string, ...actions: string[]): Promise<string | undefined>;
  log(line: string): void;
  /** Opens "Russet: Change WSL Limits" and settles once the owner is done with it. */
  changeLimits?(): Promise<unknown>;
}

export class UpdateWatcher {
  private timer?: ReturnType<typeof setInterval>;
  /** A check or an update run is in flight. */
  private busy = false;
  /** Notices already shown, by kind, component and upstream commit. */
  private readonly told = new Set<string>();

  constructor(
    private readonly runner: UpdatesRunner,
    private readonly ui: UpdatesUi,
    /** Each check's answer (whether a virtual iPhone runs, for one). */
    private readonly onCheck?: (components: readonly ComponentUpdate[]) => void,
  ) {}

  /** Checks now and then every `seconds` (0 stops). */
  schedule(seconds: number): void {
    this.stop();
    if (!(seconds > 0)) {
      return;
    }
    const every = Math.max(MIN_CHECK_INTERVAL_SECONDS, seconds) * 1000;
    void this.tick();
    this.timer = setInterval(() => void this.tick(), every);
  }

  stop(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = undefined;
    }
  }

  dispose(): void {
    this.stop();
  }

  /** One check, and the update it calls for; skipped while one is in flight. */
  async tick(): Promise<void> {
    if (this.busy) {
      return;
    }
    this.busy = true;
    try {
      const components = await this.runner.updates();
      this.onCheck?.(components);
      for (const c of components) {
        await this.consider(c);
      }
    } catch (err) {
      this.ui.log(`update check: ${(err as Error).message}`);
    } finally {
      this.busy = false;
    }
  }

  private once(key: string): boolean {
    if (this.told.has(key)) {
      return false;
    }
    this.told.add(key);
    return true;
  }

  private async consider(c: ComponentUpdate): Promise<void> {
    if (!c.update) {
      return;
    }
    const key = `${c.id}:${c.upstream}`;
    switch (c.staged) {
      case "building":
      case "skipped":
        // Another window's runner is building it, or the user rolled back from it.
        return;
      case "failed":
        if (this.once(`failed:${key}`)) {
          void this.ui
            .warn(
              `${c.title}: the update to ${c.upstream} failed${c.reason ? ` (${c.reason})` : ""}. The build in use (${c.current}) is unchanged.`,
              "Retry",
            )
            .then((pick) => {
              if (pick === "Retry") {
                this.told.delete(`failed:${key}`);
                void this.update(c, `${c.id}-retry-update`);
              }
            });
        }
        return;
      case "built":
        if (c.running) {
          if (this.once(`waiting:${key}`)) {
            void this.ui.info(`${c.title}: the update to ${c.upstream} is built; Russet switches to it once the virtual iPhone stops.`);
          }
          return;
        }
        await this.update(c);
        return;
      default:
        if (this.told.has(`disk-limit:${key}`)) {
          // Waits for the owner to change the limit (or free space and retry from the notice).
          return;
        }
        if (this.once(`started:${key}`)) {
          void this.ui.info(`${c.title}: updating to ${c.upstream} in the background; ${c.current} stays in use until it is built.`);
        }
        await this.update(c);
    }
  }

  /** Runs `<id>-update` (or `run`), then reports a switch with Roll Back. */
  private async update(c: ComponentUpdate, run = `${c.id}-update`): Promise<void> {
    const outcome = await this.runner.run(run, {}, (e) => {
      if (e.event === "progress" || e.event === "log") {
        this.ui.log(`${run}: ${e.message}`);
      }
    });
    if (outcome.kind === "failed" && outcome.code.endsWith(".disk-limit")) {
      // Refused before building: it would grow Russet's disk past the owner's limit.
      const told = `disk-limit:${c.id}:${c.upstream}`;
      if (this.once(told)) {
        const actions = this.ui.changeLimits ? ["Change WSL Limits", "Retry"] : ["Retry"];
        void this.ui.warn(`${c.title}: ${outcome.message}`, ...actions).then(async (pick) => {
          if (pick === "Change WSL Limits") {
            await this.ui.changeLimits?.();
          }
          if (pick) {
            // The next check tries again (and tells again if it is still refused).
            this.told.delete(told);
          }
        });
      }
      return;
    }
    if (outcome.kind !== "done") {
      // The next check reports the failed build once.
      return;
    }
    const after = (await this.runner.updates()).find((x) => x.id === c.id);
    if (after && after.current === c.upstream && this.once(`switched:${c.id}:${c.upstream}`)) {
      void this.ui.info(`${c.title} updated to ${c.upstream} (from ${c.current}).`, "Roll Back").then(async (pick) => {
        if (pick === "Roll Back") {
          const back = await this.runner.run(`${c.id}-rollback`, {}, (e) => {
            if (e.event === "progress" || e.event === "log") {
              this.ui.log(`${c.id}-rollback: ${e.message}`);
            }
          });
          if (back.kind === "failed") {
            void this.ui.warn(`${c.title}: ${back.message}`);
          }
        }
      });
    }
  }
}
