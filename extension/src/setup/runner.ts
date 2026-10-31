// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Spawns the Windows supervisor's setup runner (`RUSSET_WINDOWS_HOST --setup …`)
// and turns its output into typed events. The extension never shells into WSL
// itself. Cancel kills the process; steps are re-runnable by contract.

import { spawn as nodeSpawn, type SpawnOptions } from "node:child_process";
import * as path from "node:path";
import type { Readable } from "node:stream";
import {
  parseRunEvent,
  parseStatus,
  runArgv,
  SetupProtocolError,
  type SetupRunEvent,
  type SetupStatus,
} from "./contract";

/** The subset of ChildProcess the runner uses (a fake runner implements it in tests). */
export interface RunnerProcess {
  readonly stdout: Readable | null;
  readonly stderr: Readable | null;
  kill(): boolean;
  once(event: "exit", listener: (code: number | null) => void): this;
  once(event: "error", listener: (err: Error) => void): this;
}

export type SpawnFn = (file: string, args: string[], options: SpawnOptions) => RunnerProcess;

export interface RunnerCommand {
  /** The executable. */
  file: string;
  /** Arguments placed before `--setup` (a script path when the runner is a Node script). */
  prefix: string[];
  /** Extra environment for the child. */
  env?: Record<string, string>;
}

export type SetupRunOutcome =
  | { kind: "done" }
  | { kind: "failed"; code: string; message: string; retryable: boolean }
  | { kind: "cancelled" };

const MAX_LINE = 64 * 1024;
const STATUS_TIMEOUT_MS = 30_000;

export class SetupRunner {
  constructor(
    readonly command: RunnerCommand,
    private readonly spawnFn: SpawnFn = nodeSpawn as unknown as SpawnFn,
  ) {}

  /**
   * The runner for this host: `RUSSET_SETUP_RUNNER` (a development/test override:
   * an absolute `.exe`, or a `.js`/`.cjs`/`.mjs` script run with this process's
   * Node), else `RUSSET_WINDOWS_HOST` on Windows. Undefined elsewhere.
   */
  static fromEnvironment(
    env: NodeJS.ProcessEnv = process.env,
    platform: NodeJS.Platform = process.platform,
    execPath: string = process.execPath,
  ): SetupRunner | undefined {
    const override = env.RUSSET_SETUP_RUNNER;
    if (override && path.isAbsolute(override)) {
      if (/\.(c|m)?js$/i.test(override)) {
        return new SetupRunner({ file: execPath, prefix: [override], env: { ELECTRON_RUN_AS_NODE: "1" } });
      }
      return new SetupRunner({ file: override, prefix: [] });
    }
    const host = env.RUSSET_WINDOWS_HOST;
    if (platform === "win32" && host && /^[A-Za-z]:\\/.test(host) && path.win32.isAbsolute(host)) {
      return new SetupRunner({ file: host, prefix: [] });
    }
    return undefined;
  }

  private start(args: string[]): RunnerProcess {
    return this.spawnFn(this.command.file, [...this.command.prefix, ...args], {
      windowsHide: true,
      shell: false,
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, ...(this.command.env ?? {}) },
    });
  }

  /** `--setup status`: one JSON line. */
  status(timeoutMs = STATUS_TIMEOUT_MS): Promise<SetupStatus> {
    return new Promise((resolve, reject) => {
      let child: RunnerProcess;
      try {
        child = this.start(["--setup", "status"]);
      } catch (err) {
        reject(unavailable(err));
        return;
      }
      let settled = false;
      let buffered = "";
      const finish = (fn: () => void): void => {
        if (settled) {
          return;
        }
        settled = true;
        clearTimeout(timer);
        fn();
      };
      const timer = setTimeout(() => {
        child.kill();
        finish(() => reject(new SetupProtocolError("The setup runner did not report its status in time.")));
      }, timeoutMs);
      child.stderr?.resume();
      child.stdout?.setEncoding("utf8");
      child.stdout?.on("data", (chunk: string) => {
        buffered += chunk;
        const nl = buffered.indexOf("\n");
        if (nl >= 0 || buffered.length > MAX_LINE) {
          const line = nl >= 0 ? buffered.slice(0, nl) : buffered;
          finish(() => {
            try {
              resolve(parseStatus(line.replace(/\r$/, "")));
            } catch (err) {
              reject(err);
            }
          });
        }
      });
      child.once("error", (err) => finish(() => reject(unavailable(err))));
      child.once("exit", (code) => {
        finish(() => {
          if (buffered.trim().length > 0) {
            try {
              resolve(parseStatus(buffered.trim()));
            } catch (err) {
              reject(err);
            }
            return;
          }
          reject(new SetupProtocolError(`The setup runner exited (code ${String(code)}) without a status.`));
        });
      });
    });
  }

  /**
   * `--setup run <id> [--arg k=v]…`. Every event is passed to `onEvent`; resolves
   * with the step's terminal outcome. Aborting kills the runner.
   */
  run(
    id: string,
    args: Readonly<Record<string, string>>,
    onEvent: (event: SetupRunEvent) => void,
    signal?: AbortSignal,
  ): Promise<SetupRunOutcome> {
    const argv = runArgv(id, args);
    return new Promise((resolve) => {
      if (signal?.aborted) {
        resolve({ kind: "cancelled" });
        return;
      }
      let child: RunnerProcess;
      try {
        child = this.start(argv);
      } catch (err) {
        resolve(failure(id, "runner-unavailable", unavailable(err).message, true));
        return;
      }
      let outcome: SetupRunOutcome | undefined;
      let settled = false;
      let buffered = "";
      const settle = (value: SetupRunOutcome): void => {
        if (settled) {
          return;
        }
        settled = true;
        signal?.removeEventListener("abort", onAbort);
        resolve(value);
      };
      const onAbort = (): void => {
        outcome = { kind: "cancelled" };
        child.kill();
        settle(outcome);
      };
      signal?.addEventListener("abort", onAbort, { once: true });

      const handleLine = (line: string): void => {
        if (outcome) {
          return;
        }
        try {
          const event = parseRunEvent(line.replace(/\r$/, ""), id);
          if (!event) {
            return;
          }
          onEvent(event);
          if (event.event === "done") {
            outcome = { kind: "done" };
          } else if (event.event === "failed") {
            outcome = { kind: "failed", code: event.code, message: event.message, retryable: event.retryable };
          }
        } catch (err) {
          outcome = failure(id, "protocol", err instanceof Error ? err.message : String(err), true);
          child.kill();
        }
      };

      child.stderr?.resume();
      child.stdout?.setEncoding("utf8");
      child.stdout?.on("data", (chunk: string) => {
        buffered += chunk;
        let nl: number;
        while ((nl = buffered.indexOf("\n")) >= 0) {
          const line = buffered.slice(0, nl);
          buffered = buffered.slice(nl + 1);
          handleLine(line);
        }
        if (buffered.length > MAX_LINE) {
          buffered = "";
          handleLine("{\"oversized\":true}");
        }
      });
      child.once("error", (err) => {
        settle(outcome ?? failure(id, "runner-unavailable", unavailable(err).message, true));
      });
      child.once("exit", (code) => {
        if (buffered.trim().length > 0) {
          handleLine(buffered);
          buffered = "";
        }
        settle(
          outcome ??
            failure(id, "interrupted", `The step stopped before it finished (exit code ${String(code)}).`, true),
        );
      });
    });
  }
}

function failure(id: string, reason: string, message: string, retryable: boolean): SetupRunOutcome {
  return { kind: "failed", code: `setup.${id}.${reason}`, message, retryable };
}

function unavailable(err: unknown): Error {
  const code = (err as { code?: string })?.code;
  return new SetupProtocolError(
    code === "ENOENT"
      ? "The Russet setup runner is not installed. Reinstall or repair Russet."
      : "The Russet setup runner could not be started.",
  );
}
