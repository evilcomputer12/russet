// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The "Virtual iPhone" editor tab (W19): the device frame from the mirror view
// with touch/keyboard capture and hardware buttons, each wired to a
// `russet.virtual.*` command. Display frames and input delivery need the
// engine's virtual device provider (W31 interface over QMP and vphoned); until
// it exists, VirtualDeviceBackend is a stub that says so and nothing is sent.

import * as vscode from "vscode";
import { localResourceRoots, renderPage } from "./webviewHost";
import type { DestinationModel, RunDestination } from "./destinations";
import { deviceModel, type VirtualIphoneChoice } from "../setup/virtualCatalog";

export type HardwareButton = "home" | "lock" | "volumeUp" | "volumeDown";

export type VirtualInput =
  | { type: "touch"; phase: "down" | "move" | "up"; x: number; y: number }
  | { type: "key"; key: string; code: string; shift: boolean }
  | { type: "button"; button: HardwareButton }
  | { type: "orientation"; degrees: 0 | 90 | 180 | 270 };

export interface DeliveryResult {
  delivered: boolean;
  reason?: string;
}

/** Where virtual-iPhone input and screenshots go. */
export interface VirtualDeviceBackend {
  send(target: RunDestination, input: VirtualInput): Promise<DeliveryResult>;
}

/** The stand-in until the daemon's virtual device provider lands. */
export class PendingVirtualBackend implements VirtualDeviceBackend {
  static readonly reason =
    "Not delivered: the engine's virtual device provider (display and input over QMP/vphoned) is not connected yet.";

  async send(): Promise<DeliveryResult> {
    return { delivered: false, reason: PendingVirtualBackend.reason };
  }
}

type Orientation = 0 | 90 | 180 | 270;

export interface VirtualSetupInfo {
  created: boolean;
  /** Steps the runner still needs before a VM can exist (e.g. the vphone build). */
  pendingStep?: string;
  choice: VirtualIphoneChoice;
}

const NOTE_INTERVAL_MS = 250;

export class VirtualIphonePanel implements vscode.Disposable {
  public static readonly viewType = "russet.virtualIphone";

  private panel?: vscode.WebviewPanel;
  private readonly disposables: vscode.Disposable[] = [];
  private orientation: Orientation = 0;
  private captured = 0;
  private lastReason?: string;
  private noteTimer?: ReturnType<typeof setTimeout>;
  private pendingScreenshot?: (dataUrl: string | undefined) => void;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly destinations: DestinationModel,
    private readonly setupInfo: () => VirtualSetupInfo,
    private readonly backend: VirtualDeviceBackend = new PendingVirtualBackend(),
  ) {
    this.disposables.push(destinations.onDidChange(() => this.postState()));
  }

  get isOpen(): boolean {
    return !!this.panel;
  }

  show(): void {
    if (this.panel) {
      this.panel.reveal();
      return;
    }
    const panel = vscode.window.createWebviewPanel(
      VirtualIphonePanel.viewType,
      "Virtual iPhone",
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: false },
      { enableScripts: true, localResourceRoots: localResourceRoots(this.extensionUri), retainContextWhenHidden: true },
    );
    this.attach(panel);
  }

  /** Adopt a panel (new, or restored by the serializer after a reload). */
  attach(panel: vscode.WebviewPanel): void {
    this.panel = panel;
    panel.webview.options = { enableScripts: true, localResourceRoots: localResourceRoots(this.extensionUri) };
    panel.iconPath = vscode.Uri.joinPath(this.extensionUri, "media", "russet.svg");
    panel.webview.html = renderPage(panel.webview, this.extensionUri, { page: "device", title: "Virtual iPhone" });
    panel.webview.onDidReceiveMessage((msg: unknown) => void this.onMessage(msg), undefined, this.disposables);
    panel.onDidDispose(() => {
      this.panel = undefined;
      void vscode.commands.executeCommand("setContext", "russet.virtualIphoneOpen", false);
    }, undefined, this.disposables);
    void vscode.commands.executeCommand("setContext", "russet.virtualIphoneOpen", true);
  }

  /** The destination the tab shows: the selected virtual iPhone, else any virtual one. */
  private target(): RunDestination | undefined {
    const sel = this.destinations.selected;
    if (sel?.kind === "virtual") {
      return sel;
    }
    return this.destinations.all.find((d) => d.kind === "virtual");
  }

  private postState(): void {
    if (!this.panel) {
      return;
    }
    const t = this.target();
    const info = this.setupInfo();
    const choice = info.choice;
    const model = deviceModel(t?.productType ?? choice.device);
    const name = t?.name ?? model?.name ?? "Virtual iPhone";
    const os = t?.osVersion ?? choice.ios;
    this.panel.title = `Virtual iPhone — ${name}`;
    let status: { title: string; detail?: string; action?: { id: string; label: string } } | undefined;
    let state: string;
    if (t?.ready) {
      state = "Running";
      status = undefined;
    } else if (t || info.created) {
      state = "Not running";
      status = {
        title: `${name} is not running`,
        detail: "Starting the virtual iPhone and streaming its screen from the IDE arrive with the engine's virtual device provider.",
      };
    } else {
      state = "Not created";
      status = {
        title: "No virtual iPhone yet",
        detail: info.pendingStep
          ? `Russet creates it in the guided setup (next: ${info.pendingStep}).`
          : "Russet creates it in the guided setup; it takes a few hours the first time.",
        action: { id: "createVirtual", label: "Create Virtual iPhone…" },
      };
    }
    void this.panel.webview.postMessage({
      kind: "state",
      state: {
        device: {
          name,
          detail: `iOS ${os} · Virtual · ${state}`,
          screen: t?.screen ?? model?.screen ?? [1206, 2622],
          kind: "virtual",
        },
        orientation: this.orientation,
        ...(status ? { status } : {}),
        note: this.note(),
      },
    });
  }

  private note(): string {
    if (this.captured === 0) {
      return "Click or drag on the screen to touch; type while it has focus. Escape leaves the screen.";
    }
    return `${this.captured} input event${this.captured === 1 ? "" : "s"} captured. ${this.lastReason ?? "Delivered."}`;
  }

  private scheduleNote(): void {
    if (this.noteTimer) {
      return;
    }
    this.noteTimer = setTimeout(() => {
      this.noteTimer = undefined;
      this.postState();
    }, NOTE_INTERVAL_MS);
  }

  private async deliver(input: VirtualInput): Promise<void> {
    this.captured++;
    const target = this.target();
    const result: DeliveryResult = target
      ? await this.backend.send(target, input)
      : { delivered: false, reason: "Not delivered: there is no virtual iPhone yet." };
    this.lastReason = result.delivered ? undefined : result.reason;
    this.scheduleNote();
  }

  /** Hardware buttons (commands `russet.virtual.home|lock|volumeUp|volumeDown`). */
  async button(button: HardwareButton): Promise<void> {
    if (!this.panel) {
      this.show();
    }
    await this.deliver({ type: "button", button });
    const label = { home: "Home", lock: "Lock", volumeUp: "Volume up", volumeDown: "Volume down" }[button];
    void this.panel?.webview.postMessage({ kind: "announce", text: `${label} pressed. ${this.lastReason ?? ""}` });
  }

  async rotate(direction: "left" | "right"): Promise<void> {
    if (!this.panel) {
      this.show();
    }
    this.orientation = (((this.orientation + (direction === "right" ? 90 : 270)) % 360) as Orientation);
    await this.deliver({ type: "orientation", degrees: this.orientation });
    this.postState();
    const turn = this.orientation === 0 ? "portrait" : this.orientation === 180 ? "upside down" : "landscape";
    void this.panel?.webview.postMessage({ kind: "announce", text: `Rotated to ${turn}.` });
  }

  /** Save the current screen as PNG (command `russet.virtual.screenshot`). */
  async screenshot(): Promise<void> {
    if (!this.panel) {
      this.show();
      return;
    }
    const dataUrl = await new Promise<string | undefined>((resolve) => {
      const timer = setTimeout(() => resolve(undefined), 3000);
      this.pendingScreenshot = (value) => {
        clearTimeout(timer);
        resolve(value);
      };
      void this.panel?.webview.postMessage({ kind: "captureScreenshot" });
    });
    this.pendingScreenshot = undefined;
    const png = dataUrl?.startsWith("data:image/png;base64,") ? Buffer.from(dataUrl.slice(22), "base64") : undefined;
    if (!png) {
      void vscode.window.showInformationMessage(
        "No screen image yet: the virtual iPhone's display stream is not connected.",
      );
      return;
    }
    const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
    const uri = await vscode.window.showSaveDialog({
      title: "Save Screenshot",
      saveLabel: "Save Screenshot",
      filters: { "PNG image": ["png"] },
      defaultUri: vscode.workspace.workspaceFolders?.[0]
        ? vscode.Uri.joinPath(vscode.workspace.workspaceFolders[0].uri, `Screenshot ${stamp}.png`)
        : undefined,
    });
    if (uri) {
      await vscode.workspace.fs.writeFile(uri, png);
    }
  }

  private async onMessage(msg: unknown): Promise<void> {
    if (!msg || typeof msg !== "object") {
      return;
    }
    const m = msg as Record<string, unknown>;
    switch (m.kind) {
      case "ready":
        this.postState();
        break;
      case "input": {
        const input = parseInput(m.input);
        if (input) {
          await this.deliver(input);
        }
        break;
      }
      case "button":
        if (typeof m.name === "string") {
          await vscode.commands.executeCommand(`russet.virtual.${m.name}`);
        }
        break;
      case "action":
        if (m.id === "createVirtual") {
          await vscode.commands.executeCommand("russet.openSetup", "virtual-iphone");
        }
        break;
      case "screenshot":
        this.pendingScreenshot?.(typeof m.dataUrl === "string" ? m.dataUrl : undefined);
        break;
      default:
        break;
    }
  }

  dispose(): void {
    if (this.noteTimer) {
      clearTimeout(this.noteTimer);
    }
    this.panel?.dispose();
    for (const d of this.disposables.splice(0)) {
      d.dispose();
    }
  }
}

const BUTTONS: ReadonlySet<string> = new Set(["home", "lock", "volumeUp", "volumeDown"]);

/** Validate an input event from the page. */
export function parseInput(raw: unknown): VirtualInput | undefined {
  if (!raw || typeof raw !== "object") {
    return undefined;
  }
  const r = raw as Record<string, unknown>;
  const unit = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v) && v >= 0 && v <= 1;
  if (r.type === "touch" && (r.phase === "down" || r.phase === "move" || r.phase === "up") && unit(r.x) && unit(r.y)) {
    return { type: "touch", phase: r.phase, x: r.x, y: r.y };
  }
  if (r.type === "key" && typeof r.key === "string" && r.key.length <= 32 && typeof r.code === "string" && r.code.length <= 32) {
    return { type: "key", key: r.key, code: r.code, shift: r.shift === true };
  }
  if (r.type === "button" && typeof r.button === "string" && BUTTONS.has(r.button)) {
    return { type: "button", button: r.button as HardwareButton };
  }
  return undefined;
}
