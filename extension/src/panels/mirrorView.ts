// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The embedded "iPhone" sidebar view (ide.md §5.10): a compact device frame for
// the run destination, physical or virtual. The interactive screen is the
// "Virtual iPhone" editor tab (virtualIphonePanel.ts), which shares the frame.
// A physical iPhone is mirrored by polling `screen.screenshot` (the DVT
// screenshot service, a few frames a second) while this view is visible; the
// HEVC stream and HID input land with EXT-09 (screen.mirror.start →
// ws://127.0.0.1). The message contract is backend-neutral (device vs virtual).

import * as vscode from "vscode";
import type { RussetClient } from "../daemon/client";
import { errorDataOf, messageOf } from "../daemon/errors";
import type { Store } from "../state/store";
import type { DestinationModel } from "./destinations";
import { renderPage, localResourceRoots } from "./webviewHost";

/** The shortest time between two screenshot requests (at most about 8 frames a second). */
const FRAME_INTERVAL_MS = 125;

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

export interface MirrorTarget {
  udid: string;
  name: string;
  /** "device" for a USB/Wi-Fi iPhone; "virtual" for the embedded virtual iPhone. */
  kind: "device" | "virtual";
  display?: { width: number; height: number; scale: number };
}

export class MirrorViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewId = "russet.mirror";

  private view?: vscode.WebviewView;
  private destinations?: DestinationModel;
  private client?: RussetClient;
  private polling = false;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly store: Store,
  ) {
    store.onDidChangeActiveDevice(() => this.pushState());
  }

  /** Follow the Xcode-style destination picker (physical and virtual). */
  useDestinations(destinations: DestinationModel): void {
    this.destinations = destinations;
    destinations.onDidChange(() => this.pushState());
    this.pushState();
  }

  /** Mirror a physical run destination through the daemon. */
  useClient(client: RussetClient): void {
    this.client = client;
    this.pushState();
  }

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    view.webview.options = {
      enableScripts: true,
      localResourceRoots: localResourceRoots(this.extensionUri),
    };
    view.webview.html = renderPage(view.webview, this.extensionUri, {
      page: "mirror",
      title: "iPhone",
    });
    view.webview.onDidReceiveMessage((msg: unknown) => {
      const kind = msg && typeof msg === "object" ? (msg as { kind?: string }).kind : undefined;
      if (kind === "ready") {
        this.pushState();
      } else if (kind === "openEditor") {
        void vscode.commands.executeCommand("russet.openVirtualIphone");
      }
    });
    view.onDidChangeVisibility(() => this.mirror());
    view.onDidDispose(() => {
      this.view = undefined;
    });
  }

  async reveal(): Promise<void> {
    await vscode.commands.executeCommand("russet.mirror.focus");
    this.pushState();
  }

  private target(): (MirrorTarget & { detail: string; screen?: [number, number] }) | undefined {
    const dest = this.destinations?.selected;
    if (dest) {
      return {
        udid: dest.id,
        name: dest.name,
        kind: dest.kind === "virtual" ? "virtual" : "device",
        detail: [dest.osVersion ? `iOS ${dest.osVersion}` : "", dest.stateLabel].filter(Boolean).join(" · "),
        ...(dest.screen ? { screen: dest.screen } : {}),
      };
    }
    const d = this.store.activeDevice;
    if (!d) {
      return undefined;
    }
    return { udid: d.udid, name: d.marketingName ?? d.name, kind: "device", detail: `iOS ${d.osVersion}` };
  }

  private pushState(): void {
    const target = this.target();
    const mirrored = target?.kind === "device" && this.client !== undefined;
    void this.view?.webview.postMessage({
      kind: "state",
      // A physical iPhone is mirrored by screenshots; a virtual one waits for the stream (EXT-09).
      status: mirrored ? "mirroring" : "placeholder",
      target: target
        ? {
            udid: target.udid,
            name: target.name,
            kind: target.kind,
            detail: target.detail,
            ...(target.screen ? { screen: target.screen } : {}),
          }
        : undefined,
    });
    this.mirror();
  }

  /** Starts the screenshot loop unless it runs; it stops by itself when there is nothing to show. */
  private mirror(): void {
    if (!this.polling) {
      void this.poll();
    }
  }

  private async poll(): Promise<void> {
    this.polling = true;
    let failures = 0;
    try {
      for (;;) {
        const target = this.target();
        if (!this.view?.visible || !this.client || target?.kind !== "device") {
          return;
        }
        const started = Date.now();
        try {
          const shot = await this.client.call("screen.screenshot", { udid: target.udid });
          failures = 0;
          if (shot.imageBase64) {
            void this.view?.webview.postMessage({
              kind: "frame",
              udid: target.udid,
              png: shot.imageBase64,
              width: shot.width,
              height: shot.height,
            });
          }
        } catch (err) {
          failures += 1;
          void this.view?.webview.postMessage({
            kind: "mirrorStatus",
            udid: target.udid,
            title: "No screen",
            detail: messageOf(err),
          });
          if (errorDataOf(err)?.russetCode === "core.notImplemented") {
            return;
          }
          await delay(Math.min(2000 * failures, 10000));
        }
        await delay(Math.max(0, FRAME_INTERVAL_MS - (Date.now() - started)));
      }
    } finally {
      this.polling = false;
    }
  }
}
