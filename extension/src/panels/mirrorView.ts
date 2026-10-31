// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The embedded "iPhone" sidebar view (ide.md §5.10): a device frame for the run
// destination, physical or virtual. The virtual iPhone's interactive screen is
// the "Virtual iPhone" editor tab (virtualIphonePanel.ts), which shares the frame.
// A physical iPhone streams while this view is visible: `screen.mirror.start`
// opens the device's HEVC display session, and the webview decodes it from
// ws://127.0.0.1 (ARCHITECTURE §10.1) and sends touch, keys and buttons back.
// When the stream cannot start or the webview cannot decode HEVC, the view polls
// `screen.screenshot` instead (a few frames a second) and retries the stream
// later. The message contract is backend-neutral (device vs virtual).

import * as vscode from "vscode";
import type { RussetClient } from "../daemon/client";
import { errorDataOf, messageOf } from "../daemon/errors";
import type { Store } from "../state/store";
import type { DestinationModel } from "./destinations";
import { renderPage, localResourceRoots } from "./webviewHost";

/** The shortest time between two screenshot requests (at most about 8 frames a second). */
const FRAME_INTERVAL_MS = 125;
/** After a stream fails, poll screenshots this long before trying the stream again. */
const STREAM_RETRY_MS = 15_000;
/** After the webview reports it cannot decode HEVC, keep polling this long. */
const STREAM_UNSUPPORTED_MS = 60 * 60_000;

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
  /** The device whose stream the webview has, or is being started. */
  private streamUdid?: string;
  private streamId?: string;
  /** Devices whose stream failed, until when (screenshots meanwhile). */
  private readonly streamBlocked = new Map<string, number>();

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
      // The daemon's web endpoint picks its port when it starts.
      mirrorPort: "any",
    });
    view.webview.onDidReceiveMessage((msg: unknown) => {
      const kind = msg && typeof msg === "object" ? (msg as { kind?: string }).kind : undefined;
      if (kind === "ready") {
        // A fresh webview (first load, or shown again after being hidden): any earlier stream died with the old one.
        this.forgetStream();
        this.pushState();
      } else if (kind === "streamEnded") {
        const m = msg as { udid?: string; unsupported?: boolean };
        if (m.udid && m.udid === this.streamUdid) {
          this.forgetStream();
          this.streamBlocked.set(m.udid, Date.now() + (m.unsupported ? STREAM_UNSUPPORTED_MS : STREAM_RETRY_MS));
          this.mirror();
        }
      } else if (kind === "openEditor") {
        void vscode.commands.executeCommand("russet.openVirtualIphone");
      }
    });
    view.onDidChangeVisibility(() => this.mirror());
    view.onDidDispose(() => {
      this.stopStream();
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

  /**
   * Streams the physical destination while the view is visible, or polls screenshots while its stream is blocked;
   * the screenshot loop stops by itself when there is nothing to show or a stream takes over.
   */
  private mirror(): void {
    const target = this.target();
    if (!this.view?.visible || !this.client || target?.kind !== "device") {
      this.stopStream();
      return;
    }
    if (this.streamUdid === target.udid) {
      return;
    }
    if (Date.now() >= (this.streamBlocked.get(target.udid) ?? 0)) {
      void this.startStream(target.udid);
    } else if (!this.polling) {
      void this.poll();
    }
  }

  private async startStream(udid: string): Promise<void> {
    const client = this.client;
    if (!client) {
      return;
    }
    this.stopStream();
    this.streamUdid = udid;
    try {
      const s = await client.call("screen.mirror.start", { udid, codecs: ["hevc"], input: true });
      if (this.streamUdid !== udid || !this.view) {
        void client.call("screen.mirror.stop", { streamId: s.streamId }).catch(() => undefined);
        return;
      }
      this.streamId = s.streamId;
      void this.view.webview.postMessage({
        kind: "stream",
        udid,
        url: s.url,
        token: s.token,
        width: s.width,
        height: s.height,
      });
    } catch (err) {
      if (this.streamUdid !== udid) {
        return;
      }
      this.streamUdid = undefined;
      const code = errorDataOf(err)?.russetCode ?? "";
      const unsupported = code === "core.notImplemented" || code === "screen.codecUnsupported";
      this.streamBlocked.set(udid, Date.now() + (unsupported ? STREAM_UNSUPPORTED_MS : STREAM_RETRY_MS));
      this.mirror();
    }
  }

  /** Ends the stream, if any: the daemon closes the display session, the webview its socket. */
  private stopStream(): void {
    if (this.streamUdid === undefined) {
      return;
    }
    const streamId = this.streamId;
    this.forgetStream();
    if (streamId) {
      void this.client?.call("screen.mirror.stop", { streamId }).catch(() => undefined);
    }
    void this.view?.webview.postMessage({ kind: "stopStream" });
  }

  private forgetStream(): void {
    this.streamUdid = undefined;
    this.streamId = undefined;
  }

  private async poll(): Promise<void> {
    this.polling = true;
    let failures = 0;
    try {
      for (;;) {
        const target = this.target();
        if (!this.view?.visible || !this.client || target?.kind !== "device" || this.streamUdid === target.udid) {
          return;
        }
        if (Date.now() >= (this.streamBlocked.get(target.udid) ?? 0)) {
          // Time to try the stream again; mirror() starts it once this loop has ended.
          setTimeout(() => this.mirror(), 0);
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
