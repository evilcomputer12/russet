// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Debug on the virtual iPhone: the runner's `virtual-iphone-debug` result (the app launched under
// the phone's debugserver, attached, and the backend's sockets for LLDB and lldb-dap) and the
// lldb-dap attach configuration built from it. No vscode import, so it is unit-tested.

/** A `russet-virtual` debug configuration (vscode.DebugConfiguration's shape). */
export interface VirtualDebugConfiguration {
  type: "russet-virtual";
  request: "attach";
  name: string;
  __dapSocket: string;
  initCommands: string[];
  attachCommands: string[];
}

/** What `virtual-iphone-debug` reports: Linux paths in the backend and the phone. */
export interface VirtualDebugSession {
  pid: number;
  executable: string;
  appDir: string;
  gdbSocket: string;
  dapSocket: string;
  localApp: string;
}

export function virtualDebugSession(result: Record<string, unknown> | undefined): VirtualDebugSession | undefined {
  const text = (k: string): string | undefined => (typeof result?.[k] === "string" ? (result[k] as string) : undefined);
  const [executable, appDir, gdbSocket, dapSocket, localApp] = ["executable", "appDir", "gdbSocket", "dapSocket", "localApp"].map(text);
  const managed = (p: string | undefined): p is string => !!p && p.startsWith("/home/russet/") && !p.includes("/../");
  if (typeof result?.pid !== "number" || !executable || !appDir || !localApp || !managed(gdbSocket) || !managed(dapSocket)) {
    return undefined;
  }
  return { pid: result.pid, executable, appDir, gdbSocket, dapSocket, localApp };
}

/** lldb-dap's attach recipe for it: the steps Russet's engine gives LLDB for a real iPhone. */
export function virtualDebugConfiguration(s: VirtualDebugSession): VirtualDebugConfiguration {
  const quote = (p: string): string => `"${p.replace(/(["\\])/g, "\\$1")}"`;
  const name = s.executable.slice(s.executable.lastIndexOf("/") + 1);
  return {
    type: "russet-virtual",
    request: "attach",
    name: "Debug on Virtual iPhone",
    __dapSocket: s.dapSocket,
    initCommands: [
      // The phone's own libraries are read from its memory: only what a stop needs.
      "settings set target.memory-module-load-level minimal",
      "settings set plugin.process.gdb-remote.packet-timeout 60",
    ],
    attachCommands: [
      "platform select remote-ios",
      `target create --arch arm64-apple-ios ${quote(`${s.localApp}/${name}`)}`,
      `target modules search-paths add ${quote(s.appDir)} ${quote(s.localApp)}`,
      `process connect -p gdb-remote unix-connect://${s.gdbSocket}`,
    ],
  };
}
