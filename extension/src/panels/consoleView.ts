// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The Device Console webview (EXT-08): a performant, filterable log view fed by
// run.output / logs.entries. Untrusted log text is rendered as text in the webview.

import * as vscode from "vscode";
import type { LogEntry } from "../generated/protocol";
import { renderPage, localResourceRoots } from "./webviewHost";

export class ConsoleViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewId = "russet.console";

  private view?: vscode.WebviewView;
  private ready = false;
  private readonly pending: LogEntry[] = [];
  private activePid?: number;

  constructor(private readonly extensionUri: vscode.Uri) {}

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    view.webview.options = {
      enableScripts: true,
      localResourceRoots: localResourceRoots(this.extensionUri),
    };
    view.webview.html = renderPage(view.webview, this.extensionUri, {
      page: "console",
      title: "Device Console",
    });
    view.webview.onDidReceiveMessage((msg: unknown) => this.onMessage(msg));
    view.onDidDispose(() => {
      this.view = undefined;
      this.ready = false;
    });
  }

  private onMessage(msg: unknown): void {
    if (!msg || typeof msg !== "object") {
      return;
    }
    const m = msg as { kind?: string; id?: number; t?: number; text?: string };
    switch (m.kind) {
      case "ready":
        this.ready = true;
        this.post({
          kind: "init",
          maxLines: vscode.workspace.getConfiguration("russet").get<number>("console.maxLines", 50000),
        });
        if (this.pending.length) {
          this.post({ kind: "append", entries: this.pending.splice(0) });
        }
        break;
      case "perfPong":
        // The webview echoed a perf ping; the test-only hook reads it back.
        this.post({ kind: "perfAck", id: m.id });
        break;
      case "export":
        void this.exportBuffer(m.text ?? "");
        break;
      default:
        break;
    }
  }

  private post(message: unknown): void {
    void this.view?.webview.postMessage(message);
  }

  /** Focus the console panel and filter to a running pid. */
  async reveal(pid?: number): Promise<void> {
    this.activePid = pid;
    await vscode.commands.executeCommand("russet.console.focus");
    if (pid !== undefined) {
      this.post({ kind: "filter", pid });
    }
  }

  append(entries: LogEntry[]): void {
    if (entries.length === 0) {
      return;
    }
    if (!this.ready) {
      this.pending.push(...entries);
      return;
    }
    this.post({ kind: "append", entries });
  }

  /** Append raw stdout chunks (app.output / run.output) as default-level lines. */
  appendRaw(lines: string[], source: LogEntry["source"] = "stdout"): void {
    const ts = new Date().toISOString();
    this.append(
      lines.map((message) => ({ ts, source, level: "default", message } as LogEntry)),
    );
  }

  clear(): void {
    this.pending.length = 0;
    this.post({ kind: "clear" });
  }

  /** Test-only hook: send a ping the webview must echo (console latency budget, §9). */
  perfPing(id: number): void {
    this.post({ kind: "perfPing", id, t: Date.now() });
  }

  get currentPid(): number | undefined {
    return this.activePid;
  }

  private async exportBuffer(text: string): Promise<void> {
    const uri = await vscode.window.showSaveDialog({
      filters: { Log: ["log", "txt"] },
      saveLabel: "Export Console Log",
    });
    if (uri) {
      await vscode.workspace.fs.writeFile(uri, Buffer.from(text, "utf8"));
    }
  }
}
