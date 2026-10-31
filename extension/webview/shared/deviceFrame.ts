// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// An iPhone frame around a screen canvas, shared by the "Virtual iPhone" editor
// tab and the sidebar mirror. In interactive mode it captures touch (pointer
// down/move/up, mapped to device coordinates for the current rotation) and
// keyboard input while the screen has focus. Tab and modified keys are never
// captured, so keyboard users can always leave and VS Code shortcuts still work;
// Escape releases focus.

import { h } from "./dom";
import { isLandscape, toDevicePoint, type Orientation } from "./geometry";

export interface FrameDevice {
  name: string;
  /** For example "iOS 27.0.1 · Virtual". */
  detail: string;
  /** Panel size in pixels, portrait. */
  screen: [number, number];
}

export type FrameInput =
  | { type: "touch"; phase: "down" | "move" | "up"; x: number; y: number }
  | { type: "key"; key: string; code: string; shift: boolean };

export type SideButton = "volumeUp" | "volumeDown" | "lock";

export interface FrameOptions {
  interactive: boolean;
  onInput?: (input: FrameInput) => void;
  onSideButton?: (button: SideButton) => void;
}

export interface FrameStatus {
  title: string;
  detail?: string;
  action?: { label: string; run: () => void };
  busy?: boolean;
}

const BEZEL_RATIO = 0.045;

export class DeviceFrame {
  readonly el: HTMLElement;
  readonly screen: HTMLElement;
  readonly canvas: HTMLCanvasElement;
  private readonly overlay: HTMLElement;
  private device: FrameDevice = { name: "iPhone", detail: "", screen: [1206, 2622] };
  private orientation: Orientation = 0;
  private hasFrame = false;
  private activePointer?: number;
  private pendingMove?: { x: number; y: number };
  private moveScheduled = false;

  constructor(private readonly parent: HTMLElement, private readonly opts: FrameOptions) {
    this.canvas = h("canvas", { class: "screen-canvas", "aria-hidden": "true" });
    this.overlay = h("div", { class: "screen-overlay", role: "status", "aria-live": "polite" });
    this.screen = h(
      "div",
      {
        class: "screen",
        ...(opts.interactive
          ? {
              tabindex: 0,
              role: "application",
              "aria-roledescription": "device screen",
              "aria-keyshortcuts": "Escape",
            }
          : { role: "img" }),
      },
      this.canvas,
      h("div", { class: "island", "aria-hidden": "true" }),
      this.overlay,
    );
    const side = (name: SideButton, label: string): HTMLElement =>
      h("button", {
        class: `side ${name}`,
        "aria-label": label,
        title: label,
        tabindex: opts.interactive ? 0 : -1,
        onclick: () => opts.onSideButton?.(name),
      });
    this.el = h(
      "div",
      { class: `device o0${opts.interactive ? "" : " compact"}` },
      opts.interactive ? side("volumeUp", "Volume up") : "",
      opts.interactive ? side("volumeDown", "Volume down") : "",
      opts.interactive ? side("lock", "Side button (lock)") : "",
      this.screen,
    );
    parent.append(this.el);
    if (opts.interactive) {
      this.installInput();
    }
    new ResizeObserver(() => this.layout()).observe(parent);
    this.layout();
  }

  setDevice(device: FrameDevice): void {
    this.device = device;
    this.updateLabel();
    this.layout();
  }

  setOrientation(orientation: Orientation): void {
    this.orientation = orientation;
    this.el.classList.remove("o0", "o90", "o180", "o270");
    this.el.classList.add(`o${orientation}`);
    this.updateLabel();
    this.layout();
  }

  setStatus(status: FrameStatus | undefined): void {
    this.overlay.replaceChildren();
    this.overlay.hidden = !status;
    if (!status) {
      return;
    }
    this.overlay.append(
      status.busy ? h("div", { class: "spin big", "aria-hidden": "true" }, "◌") : "",
      h("p", { class: "overlay-title" }, status.title),
      status.detail ? h("p", { class: "overlay-detail" }, status.detail) : "",
      status.action
        ? h("button", { class: "overlay-action", onclick: status.action.run }, status.action.label)
        : "",
    );
  }

  /** Draw one decoded frame (live display stream). */
  drawFrame(source: CanvasImageSource, width: number, height: number): void {
    if (this.canvas.width !== width || this.canvas.height !== height) {
      this.canvas.width = width;
      this.canvas.height = height;
    }
    this.canvas.getContext("2d")?.drawImage(source, 0, 0, width, height);
    this.hasFrame = true;
  }

  /** The current screen as a PNG data URL, or undefined before the first frame. */
  snapshot(): string | undefined {
    return this.hasFrame ? this.canvas.toDataURL("image/png") : undefined;
  }

  private updateLabel(): void {
    if (!this.opts.interactive) {
      this.screen.setAttribute("aria-label", `${this.device.name} screen`);
      return;
    }
    const turn = this.orientation === 0 ? "portrait" : this.orientation === 180 ? "upside down" : "landscape";
    this.screen.setAttribute(
      "aria-label",
      `${this.device.name} screen, ${turn}. Click or drag to touch; typing goes to the device. Press Escape to leave.`,
    );
  }

  /** Fit the frame into the parent, keeping the panel's aspect ratio. */
  private layout(): void {
    const [pw, ph] = this.device.screen;
    const land = isLandscape(this.orientation);
    const sw = land ? ph : pw;
    const sh = land ? pw : ph;
    const bezel = Math.max(pw, ph) * BEZEL_RATIO;
    const fw = sw + bezel * 2;
    const fh = sh + bezel * 2;
    const box = this.parent.getBoundingClientRect();
    const availW = Math.max(120, box.width - 48);
    const availH = Math.max(160, box.height - 24);
    const scale = Math.min(availW / fw, availH / fh);
    this.el.style.width = `${Math.floor(fw * scale)}px`;
    this.el.style.height = `${Math.floor(fh * scale)}px`;
    this.el.style.setProperty("--bezel", `${Math.max(6, Math.floor(bezel * scale))}px`);
    this.el.style.setProperty("--radius", `${Math.floor(Math.min(fw, fh) * scale * 0.16)}px`);
  }

  private point(ev: PointerEvent): { x: number; y: number } {
    const r = this.screen.getBoundingClientRect();
    const p = toDevicePoint(ev.clientX - r.left, ev.clientY - r.top, r.width, r.height, this.orientation);
    return { x: Math.round(p.x * 10000) / 10000, y: Math.round(p.y * 10000) / 10000 };
  }

  private installInput(): void {
    const s = this.screen;
    s.addEventListener("pointerdown", (ev) => {
      if (ev.button !== 0 || this.activePointer !== undefined) {
        return;
      }
      ev.preventDefault();
      s.focus();
      this.activePointer = ev.pointerId;
      s.setPointerCapture(ev.pointerId);
      this.opts.onInput?.({ type: "touch", phase: "down", ...this.point(ev) });
    });
    s.addEventListener("pointermove", (ev) => {
      if (ev.pointerId !== this.activePointer) {
        return;
      }
      this.pendingMove = this.point(ev);
      if (!this.moveScheduled) {
        this.moveScheduled = true;
        requestAnimationFrame(() => {
          this.moveScheduled = false;
          if (this.pendingMove && this.activePointer !== undefined) {
            this.opts.onInput?.({ type: "touch", phase: "move", ...this.pendingMove });
          }
          this.pendingMove = undefined;
        });
      }
    });
    const end = (ev: PointerEvent): void => {
      if (ev.pointerId !== this.activePointer) {
        return;
      }
      this.activePointer = undefined;
      this.pendingMove = undefined;
      this.opts.onInput?.({ type: "touch", phase: "up", ...this.point(ev) });
    };
    s.addEventListener("pointerup", end);
    s.addEventListener("pointercancel", end);
    s.addEventListener("keydown", (ev) => {
      if (ev.key === "Tab" || ev.ctrlKey || ev.metaKey || ev.altKey) {
        return;
      }
      if (ev.key === "Escape") {
        s.blur();
        return;
      }
      ev.preventDefault();
      ev.stopPropagation();
      this.opts.onInput?.({ type: "key", key: ev.key, code: ev.code, shift: ev.shiftKey });
    });
  }
}
