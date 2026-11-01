// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// AI in Russet: assistants installed from Open VSX (Claude Code, Codex, Gemini Code Assist,
// Continue for tab autocompletion with any model, Cline), and two MCP servers they can use:
// `russet` (the engine's tools: build, run, debug, devices, screenshots, logs, signing) and
// `russet-ide` (a read-only look at the IDE itself, for fixing its bugs). Registrations are
// project-scoped, as the engine's own `russet mcp install` writes them.

import * as path from "node:path";
import * as vscode from "vscode";
import { log } from "../util/log";
import { mcpServers, registrationFor, MCP_CLIENTS, type McpServer } from "./mcp";

interface Assistant {
  id: string;
  label: string;
  detail: string;
}

/** On Open VSX, the gallery this editor uses. */
export const ASSISTANTS: readonly Assistant[] = [
  { id: "anthropic.claude-code", label: "Claude Code", detail: "Anthropic's coding agent in the editor" },
  { id: "openai.chatgpt", label: "Codex", detail: "OpenAI's coding agent in the editor" },
  { id: "google.geminicodeassist", label: "Gemini Code Assist", detail: "Google's assistant with completions" },
  { id: "continue.continue", label: "Continue", detail: "Chat and tab autocompletion with Claude, GPT, Gemini, Mistral or a local model (Ollama)" },
  { id: "saoudrizwan.claude-dev", label: "Cline", detail: "Open-source agent for any model provider" },
];

function linuxPath(windowsPath: string): string {
  const m = /^([A-Za-z]):[\\/](.*)$/.exec(windowsPath);
  return m ? `/mnt/${m[1].toLowerCase()}/${m[2].replace(/\\/g, "/")}` : windowsPath.replace(/\\/g, "/");
}

export function registerAi(context: vscode.ExtensionContext, backendHome: string | undefined): void {
  const project = (): string | undefined => vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  const servers = (client: string): McpServer[] => {
    const root = project();
    return mcpServers({
      client,
      project: root ? linuxPath(root) : undefined,
      backendHome: backendHome ?? "/home/russet/.russet-dev/engine",
      editor: process.execPath,
      ideScript: path.join(context.extensionPath, "media", "ide-mcp.js"),
      ideData: process.env.RUSSET_IDE_DATA ?? path.join(process.env.LOCALAPPDATA ?? "", "Russet", "IDE-Development"),
      host: process.env.RUSSET_WINDOWS_HOST,
    });
  };

  // The editor's own chat (and agent mode) sees both servers.
  const lm = vscode.lm as unknown as {
    registerMcpServerDefinitionProvider?: (id: string, provider: { provideMcpServerDefinitions(): unknown[] }) => vscode.Disposable;
  };
  const Stdio = (vscode as unknown as { McpStdioServerDefinition?: new (label: string, command: string, args: string[], env: Record<string, string>) => unknown })
    .McpStdioServerDefinition;
  if (typeof lm.registerMcpServerDefinitionProvider === "function" && Stdio) {
    context.subscriptions.push(
      lm.registerMcpServerDefinitionProvider("russet.mcp", {
        provideMcpServerDefinitions: () => servers("vscode").map((s) => new Stdio(s.label, s.command, s.args, s.env)),
      }),
    );
  }

  const reg = (id: string, fn: (...args: unknown[]) => unknown): void => {
    context.subscriptions.push(vscode.commands.registerCommand(id, fn));
  };

  reg("russet.agents.install", async () => {
    const installed = new Set(vscode.extensions.all.map((e) => e.id.toLowerCase()));
    const picks = await vscode.window.showQuickPick(
      [
        ...ASSISTANTS.map((a) => ({ label: a.label, description: installed.has(a.id) ? "installed" : a.id, detail: a.detail, id: a.id })),
        { label: "GitHub Copilot", description: "not in Code-OSS builds", detail: "Microsoft Marketplace only; Copilot CLI can still use Russet's MCP server", id: "" },
      ],
      { title: "Install AI Assistants", canPickMany: true, placeHolder: "Choose assistants to install from Open VSX" },
    );
    const chosen = (picks ?? []).filter((p) => p.id && !installed.has(p.id));
    if ((picks ?? []).some((p) => !p.id)) {
      void vscode.window.showInformationMessage(
        "GitHub Copilot is only on Microsoft's marketplace, which Code-OSS builds such as Russet cannot use. Its CLI can use Russet's MCP server: Russet: Register Russet MCP with….",
      );
    }
    if (chosen.length === 0) {
      return;
    }
    // Assistants need the editor's AI features, which Russet's first install turned off.
    const ai = vscode.workspace.getConfiguration("chat");
    if (ai.get<boolean>("disableAIFeatures")) {
      await ai.update("disableAIFeatures", false, vscode.ConfigurationTarget.Global);
    }
    const done: string[] = [];
    await vscode.window.withProgress({ location: vscode.ProgressLocation.Notification, title: "Installing AI assistants" }, async (progress) => {
      for (const p of chosen) {
        progress.report({ message: p.label });
        try {
          // The editor asks once per publisher whether to trust it; a refusal is a cancelled install.
          await vscode.commands.executeCommand("workbench.extensions.installExtension", p.id);
          done.push(p.label);
          log().info(`[ai] installed ${p.id}`);
        } catch (err) {
          void vscode.window.showErrorMessage(`${p.label} did not install: ${(err as Error).message}`);
        }
      }
    });
    if (done.length === 0) {
      return;
    }
    const next = await vscode.window.showInformationMessage(
      `Installed ${done.join(", ")}. Give them Russet's tools (build, run, debug, devices, the IDE's logs)?`,
      "Register Russet MCP",
    );
    if (next) {
      await vscode.commands.executeCommand("russet.mcp.register");
    }
  });

  reg("russet.mcp.register", async () => {
    const root = project();
    if (!root) {
      void vscode.window.showWarningMessage("Open a project first: Russet registers its MCP servers per project.");
      return;
    }
    const picks = await vscode.window.showQuickPick(
      MCP_CLIENTS.map((c) => ({ label: c.label, description: c.file, picked: c.id === "claude", client: c })),
      { title: "Register Russet MCP", canPickMany: true, placeHolder: "Agents that should get Russet's tools in this project" },
    );
    for (const p of picks ?? []) {
      const target = vscode.Uri.file(path.join(root, p.client.file));
      let before = "";
      try {
        before = new TextDecoder().decode(await vscode.workspace.fs.readFile(target));
      } catch {
        // A new file.
      }
      const after = registrationFor(p.client.id, before, servers(p.client.id));
      await vscode.workspace.fs.createDirectory(vscode.Uri.file(path.dirname(target.fsPath)));
      await vscode.workspace.fs.writeFile(target, new TextEncoder().encode(after));
      log().info(`[ai] registered Russet MCP for ${p.client.id} in ${p.client.file}`);
    }
    if (picks?.length) {
      void vscode.window.showInformationMessage(`Russet's MCP servers are registered for ${picks.map((p) => p.label).join(", ")} in this project.`);
    }
  });

  reg("russet.mcp.copyConfig", async () => {
    await vscode.env.clipboard.writeText(registrationFor("claude", "", servers("claude")));
    void vscode.window.showInformationMessage("Copied Russet's MCP configuration (mcpServers JSON) to the clipboard.");
  });

  reg("russet.mcp.status", async () => {
    const root = project();
    const found: string[] = [];
    for (const c of MCP_CLIENTS) {
      try {
        const text = new TextDecoder().decode(await vscode.workspace.fs.readFile(vscode.Uri.file(path.join(root ?? "", c.file))));
        if (/russet/.test(text)) found.push(c.label);
      } catch {
        // Not registered.
      }
    }
    void vscode.window.showInformationMessage(
      found.length ? `Russet MCP is registered for: ${found.join(", ")}. The editor's chat always has it.` : "Russet MCP is not registered for an agent in this project yet. The editor's chat always has it.",
      "Register…",
    ).then((c) => c && vscode.commands.executeCommand("russet.mcp.register"));
  });
}
