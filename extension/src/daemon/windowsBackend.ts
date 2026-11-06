// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

// Optional Windows launcher integration. Endpoint paths come from the installed
// supervisor, never from workspace settings or a Linux socket passed to Windows.
import { spawn, type ChildProcess } from "node:child_process";
import * as path from "node:path";

export interface BackendRelay {
  endpoint: string;
  dispose(): void;
}

/** The longest a daemon start may take: the host gives `daemon start` 60 s. */
const DAEMON_START_MS = 90_000;
/** The Apple Devices bridge's start: two probes of 15 s and the helper's 20 s at most. */
const BRIDGE_START_MS = 60_000;

export class WindowsBackend {
  private readonly children = new Set<ChildProcess>();
  private disposed = false;
  private starting?: Promise<boolean>;
  /** The Apple Devices bridge this window started (enableDevices). */
  private bridge?: ChildProcess;

  constructor(
    private readonly executable: string,
    readonly home: string,
  ) {
    if (!path.win32.isAbsolute(executable) || !/^[A-Za-z]:\\/.test(executable)) {
      throw new Error("The managed backend supervisor needs an absolute Windows drive path.");
    }
    if (!home.startsWith("/home/russet/") || home.includes("/../")) {
      throw new Error("The managed backend home is invalid.");
    }
  }

  static fromEnvironment(): WindowsBackend | undefined {
    if (process.platform !== "win32" || !process.env.RUSSET_WINDOWS_HOST) return undefined;
    return new WindowsBackend(
      process.env.RUSSET_WINDOWS_HOST,
      process.env.RUSSET_BACKEND_HOME ?? "/home/russet/.russet-dev/engine",
    );
  }

  async open(socketPath = `${this.home}/runtime/russetd.sock`, protocol: "rpc" | "dap" = "rpc"): Promise<BackendRelay> {
    if (this.disposed) throw new Error("The managed backend was disposed.");
    if (!socketPath.startsWith(`${this.home}/runtime/`) || socketPath.includes("/../")) {
      throw new Error("The backend returned a socket outside its managed runtime.");
    }
    const child = spawn(this.executable, [
      "--distribution", "Russet-Dev", "--helper", "/opt/russet/lib/socket_relay.py",
      "--socket", socketPath,
      "--protocol", protocol,
    ], { windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
    this.children.add(child);
    child.once("exit", () => this.children.delete(child));
    // Drain diagnostics without ever copying protocol data or credentials to logs.
    child.stderr?.resume();
    return new Promise((resolve, reject) => {
      let output = "";
      const timer = setTimeout(() => fail(), 30_000);
      const dispose = (): void => { child.kill(); this.children.delete(child); };
      const cleanup = (): void => {
        clearTimeout(timer);
        child.stdout?.off("data", onData);
        child.off("error", fail);
        child.off("exit", fail);
      };
      const fail = (): void => {
        cleanup(); dispose();
        reject(new Error("The managed backend is unavailable. Start or repair the Russet runtime."));
      };
      const onData = (chunk: Buffer): void => {
        output += chunk.toString("utf8");
        if (output.length > 8192) { fail(); return; }
        if (!output.includes("\n")) return;
        try {
          const value = JSON.parse(output.split("\n")[0]) as { state?: string; endpoint?: string };
          if (value.state !== "ready" || !/^\\\\\.\\pipe\\russet-dev-[A-Za-z0-9-]+$/.test(value.endpoint ?? "")) {
            fail(); return;
          }
          cleanup();
          resolve({ endpoint: value.endpoint!, dispose });
        } catch { fail(); }
      };
      child.stdout?.on("data", onData);
      child.once("error", fail);
      child.once("exit", fail);
    });
  }

  /**
   * Starts the daemon in WSL as the launcher does (`--setup daemon-start`), for one that stopped
   * while Russet is open: the launcher starts it only when Russet opens. Resolves with whether it
   * runs; calls made meanwhile share the start in progress.
   */
  startDaemon(): Promise<boolean> {
    if (this.disposed) return Promise.resolve(false);
    this.starting ??= this.host(["--setup", "daemon-start"]).then((code) => {
      this.starting = undefined;
      return code === 0;
    });
    return this.starting;
  }

  /**
   * iPhones for a daemon that runs without them: Russet opened before its backend was installed (a first run),
   * so the launcher started no Apple Devices bridge. Starts one (`--device-bridge`, held until this window
   * closes), then the daemon again with iPhones (`--setup daemon-devices`). "busy" while the daemon builds or
   * runs something (ask again later).
   */
  async enableDevices(): Promise<"enabled" | "busy" | "unavailable"> {
    if (this.disposed || !(this.bridge || (await this.startBridge()))) return "unavailable";
    // The daemon starts this window makes from now on (startDaemon) have iPhones too.
    process.env.RUSSET_DEVICE_BACKEND = "live";
    const code = await this.host(["--setup", "daemon-devices"]);
    return code === 0 ? "enabled" : code === 2 ? "busy" : "unavailable";
  }

  private startBridge(): Promise<boolean> {
    const child = spawn(this.executable, ["--device-bridge"], { windowsHide: true, stdio: ["pipe", "pipe", "ignore"] });
    this.children.add(child);
    child.once("exit", () => {
      this.children.delete(child);
      if (this.bridge === child) this.bridge = undefined;
    });
    return new Promise<boolean>((resolve) => {
      let output = "";
      let settled = false;
      const settle = (ready: boolean): void => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        if (!ready) child.kill();
        // A bridge that keeps running is this window's; "ready" and an exit is another launcher's bridge.
        else if (child.exitCode === null) this.bridge = child;
        resolve(ready);
      };
      const timer = setTimeout(() => settle(false), BRIDGE_START_MS);
      child.stdout?.on("data", (chunk: Buffer) => {
        output += chunk.toString("utf8");
        if (output.includes("\n")) settle(output.split(/\r?\n/)[0] === "ready");
      });
      child.once("error", () => settle(false));
      child.once("exit", () => settle(output.startsWith("ready")));
    });
  }

  /** Runs Russet's host with `args`; resolves with its exit code (-1 when it did not run or timed out). */
  private host(args: string[]): Promise<number> {
    return new Promise<number>((resolve) => {
      const child = spawn(this.executable, args, { windowsHide: true, stdio: "ignore" });
      this.children.add(child);
      const timer = setTimeout(() => child.kill(), DAEMON_START_MS);
      const done = (code: number): void => {
        clearTimeout(timer);
        this.children.delete(child);
        resolve(code);
      };
      child.once("error", () => done(-1));
      child.once("exit", (code) => done(code ?? -1));
    });
  }

  dispose(): void {
    this.disposed = true;
    this.bridge = undefined;
    for (const child of this.children) child.kill();
    this.children.clear();
  }
}

/** What keepDevicesEnabled reads of the daemon client. */
export interface DaemonConnection {
  readonly state: string;
  readonly hello?: { server: { deviceBackend: string } };
  onDidChangeState(listener: () => void): { dispose(): void };
}

/**
 * Each time the daemon connects without iPhones (`deviceBackend: none`), enables them
 * (WindowsBackend.enableDevices). A busy daemon is asked again a minute later; five tries at most.
 */
export function keepDevicesEnabled(
  client: DaemonConnection,
  backend: Pick<WindowsBackend, "enableDevices">,
  log: (message: string) => void,
  retryMs = 60_000,
): { dispose(): void } {
  let tries = 0;
  let running = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const check = (): void => {
    if (running || tries >= 5 || client.state !== "connected" || client.hello?.server.deviceBackend !== "none") return;
    running = true;
    tries++;
    void backend.enableDevices().then((result) => {
      running = false;
      log(`[devices] the daemon ran without iPhones; enabling them: ${result}`);
      if (result === "busy") timer = setTimeout(check, retryMs);
    });
  };
  const subscription = client.onDidChangeState(check);
  check();
  return {
    dispose: () => {
      subscription.dispose();
      clearTimeout(timer);
    },
  };
}
