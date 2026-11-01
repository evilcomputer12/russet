// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Maps a point on the on-screen device display to the device's own portrait
// coordinates (0…1 on each axis), whatever way the frame is rotated. Pure, so
// tap accuracy is unit-tested without a browser.

/** Degrees the device is turned clockwise on screen. */
export type Orientation = 0 | 90 | 180 | 270;

export interface NormalizedPoint {
  x: number;
  y: number;
}

const clamp = (v: number): number => Math.min(1, Math.max(0, v));

/**
 * `px, py` are relative to the displayed screen's top-left corner, `width,
 * height` its displayed size (already swapped in landscape).
 */
export function toDevicePoint(px: number, py: number, width: number, height: number, orientation: Orientation): NormalizedPoint {
  const u = clamp(width > 0 ? px / width : 0);
  const v = clamp(height > 0 ? py / height : 0);
  switch (orientation) {
    case 90:
      return { x: v, y: 1 - u };
    case 180:
      return { x: 1 - u, y: 1 - v };
    case 270:
      return { x: 1 - v, y: u };
    default:
      return { x: u, y: v };
  }
}

export function rotate(orientation: Orientation, direction: "left" | "right"): Orientation {
  const next = (orientation + (direction === "right" ? 90 : 270)) % 360;
  return next as Orientation;
}

export function isLandscape(orientation: Orientation): boolean {
  return orientation === 90 || orientation === 270;
}
