// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, expect, it } from "vitest";
import { mcpServers, registrationFor } from "../../src/ai/mcp";

const servers = mcpServers({
  client: "claude",
  project: "/mnt/c/Users/someone/My App",
  backendHome: "/home/russet/.russet-dev/ide-development",
  editor: "C:\\Programs\\Russet\\editor\\Russet.exe",
  ideScript: "C:\\Programs\\Russet\\editor\\resources\\app\\extensions\\russet\\media\\ide-mcp.js",
  ideData: "C:\\Users\\someone\\AppData\\Local\\Russet\\IDE-Development",
});

describe("Russet MCP registrations", () => {
  it("launches the engine's server through WSL and the IDE server through the editor's Electron", () => {
    expect(servers[0].command).toBe("C:\\Windows\\System32\\wsl.exe");
    expect(servers[0].args).toEqual([
      "-d", "Russet-Dev", "-u", "russet", "--exec", "env", "RUSSET_HOME=/home/russet/.russet-dev/ide-development",
      "/opt/russet/bin/russet-dev", "mcp", "serve", "--client", "claude", "--project", "/mnt/c/Users/someone/My App",
    ]);
    expect(servers[1]).toMatchObject({ name: "russet-ide", env: { ELECTRON_RUN_AS_NODE: "1" } });
  });

  it("merges into an agent's JSON and keeps its other servers", () => {
    const before = JSON.stringify({ mcpServers: { other: { command: "x" } }, theme: "dark" });
    const after = JSON.parse(registrationFor("claude", before, servers)) as { mcpServers: Record<string, unknown>; theme: string };
    expect(Object.keys(after.mcpServers)).toEqual(["other", "russet", "russet-ide"]);
    expect(after.theme).toBe("dark");
    const vscode = JSON.parse(registrationFor("vscode", "", servers)) as { servers: Record<string, { type: string }> };
    expect(vscode.servers.russet.type).toBe("stdio");
    expect(() => registrationFor("gemini", "{ // comment\n}", servers)).toThrow(/plain JSON/);
  });

  it("replaces Russet's Codex tables and keeps the rest", () => {
    const before = 'model = "o5"\n\n[mcp_servers.russet]\ncommand = "old"\n\n[mcp_servers.russet.env]\nA = "1"\n\n[mcp_servers.other]\ncommand = "keep"\n';
    const after = registrationFor("codex", before, servers);
    expect(after).toContain('model = "o5"');
    expect(after).toContain('[mcp_servers.other]\ncommand = "keep"');
    expect(after).not.toContain('"old"');
    expect(after).toContain('command = "C:\\\\Windows\\\\System32\\\\wsl.exe"');
    expect(after.match(/\[mcp_servers\.russet\]/g)).toHaveLength(1);
    expect(after).toContain('env = { ELECTRON_RUN_AS_NODE = "1"');
  });
});
