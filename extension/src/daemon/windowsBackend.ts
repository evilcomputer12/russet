// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

// Optional Windows launcher integration. Endpoint paths come from the installed
// supervisor, never from workspace settings or a Linux socket passed to Windows.
import { spawn, type ChildProcess } from "node:child_process";
import * as path from "node:path";

export interface BackendRelay {
  endpoint: string;
  dispose(): void;
}

export class WindowsBackend {
  private readonly children = new Set<ChildProcess>();
  private disposed = false;

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

  dispose(): void {
    this.disposed = true;
    for (const child of this.children) child.kill();
    this.children.clear();
  }
}
