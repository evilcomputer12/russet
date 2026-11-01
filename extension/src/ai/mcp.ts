// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Russet's MCP servers as Windows agents launch them, and the project files that register them
// for each agent (the same files the engine's `russet mcp install` writes). No vscode import.

export interface McpServer {
  name: "russet" | "russet-ide";
  label: string;
  command: string;
  args: string[];
  env: Record<string, string>;
}

export interface McpServerOptions {
  /** The agent, passed to `russet mcp serve --client` (its tier ceiling). */
  client: string;
  /** The project, as the backend sees it (/mnt/c/...). */
  project?: string;
  /** RUSSET_HOME of the backend the IDE uses. */
  backendHome: string;
  /** The editor's executable, whose Electron runs the IDE server as Node. */
  editor: string;
  ideScript: string;
  ideData: string;
  host?: string;
  wsl?: string;
}

export function mcpServers(o: McpServerOptions): McpServer[] {
  return [
    {
      name: "russet",
      label: "Russet",
      command: o.wsl ?? "C:\\Windows\\System32\\wsl.exe",
      args: [
        "-d", "Russet-Dev", "-u", "russet", "--exec", "env", `RUSSET_HOME=${o.backendHome}`,
        "/opt/russet/bin/russet-dev", "mcp", "serve", "--client", o.client,
        ...(o.project ? ["--project", o.project] : []),
      ],
      env: {},
    },
    {
      name: "russet-ide",
      label: "Russet IDE",
      command: o.editor,
      args: [o.ideScript],
      env: { ELECTRON_RUN_AS_NODE: "1", RUSSET_IDE_DATA: o.ideData, ...(o.host ? { RUSSET_WINDOWS_HOST: o.host } : {}) },
    },
  ];
}

export interface McpClient {
  id: string;
  label: string;
  /** The project-relative registration file. */
  file: string;
}

export const MCP_CLIENTS: readonly McpClient[] = [
  { id: "claude", label: "Claude Code", file: ".mcp.json" },
  { id: "codex", label: "Codex", file: ".codex/config.toml" },
  { id: "gemini", label: "Gemini CLI", file: ".gemini/settings.json" },
  { id: "vscode", label: "VS Code agents (Copilot, Continue, Cline)", file: ".vscode/mcp.json" },
  { id: "cursor", label: "Cursor", file: ".cursor/mcp.json" },
];

const tomlString = (s: string): string => `"${s.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;

/** `before` (the client's file, or "") with Russet's servers set; everything else is kept. */
export function registrationFor(client: string, before: string, servers: readonly McpServer[]): string {
  if (client === "codex") {
    // Drop Russet's own tables (and their sub-tables), then append them fresh.
    const kept: string[] = [];
    let skipping = false;
    for (const line of before.split(/\r?\n/)) {
      const table = /^\s*\[([^\]]+)\]\s*$/.exec(line)?.[1]?.trim();
      if (table !== undefined) {
        skipping = /^mcp_servers\.(russet|russet-ide)(\.|$)/.test(table);
      }
      if (!skipping) kept.push(line);
    }
    while (kept.length && kept[kept.length - 1].trim() === "") kept.pop();
    const blocks = servers.map((s) => {
      const env = Object.entries(s.env).map(([k, v]) => `${k} = ${tomlString(v)}`).join(", ");
      return [`[mcp_servers.${s.name}]`, `command = ${tomlString(s.command)}`, `args = [${s.args.map(tomlString).join(", ")}]`,
        ...(env ? [`env = { ${env} }`] : [])].join("\n");
    });
    return [...(kept.length ? [kept.join("\n"), ""] : []), blocks.join("\n\n"), ""].join("\n");
  }
  let doc: Record<string, unknown> = {};
  if (before.trim()) {
    try {
      doc = JSON.parse(before) as Record<string, unknown>;
    } catch {
      throw new Error("The file is not plain JSON (comments?); add Russet's servers by hand or remove the comments.");
    }
  }
  const key = client === "vscode" ? "servers" : "mcpServers";
  const table = (doc[key] && typeof doc[key] === "object" ? doc[key] : {}) as Record<string, unknown>;
  for (const s of servers) {
    table[s.name] = { ...(client === "vscode" ? { type: "stdio" } : {}), command: s.command, args: s.args, env: s.env };
  }
  doc[key] = table;
  return JSON.stringify(doc, null, 2) + "\n";
}
