// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The virtual-iPhone catalog (data/virtual-iphones.json): which device and iOS
// pairings the `virtual-iphone` setup step can create, and the default the owner
// chose (iPhone17,3 with iOS 27.0.1, verified on Linux).

import catalog from "../../data/virtual-iphones.json";

export interface VirtualIphoneChoice {
  device: string;
  ios: string;
}

export interface VirtualDeviceModel {
  id: string;
  name: string;
  /** Panel size in pixels (portrait). */
  screen: [number, number];
}

export interface VirtualPairing {
  device: string;
  ios: string;
  build: string;
  linuxTested: boolean;
}

export const DEFAULT_VIRTUAL: VirtualIphoneChoice = { ...catalog.default };

export function virtualDevices(): VirtualDeviceModel[] {
  return catalog.devices.map((d) => ({ id: d.id, name: d.name, screen: [d.screen[0], d.screen[1]] }));
}

export function pairingsFor(device: string): VirtualPairing[] {
  return catalog.pairings.filter((p) => p.device === device);
}

export function deviceModel(id: string): VirtualDeviceModel | undefined {
  return virtualDevices().find((d) => d.id === id);
}

export function deviceName(id: string): string {
  return deviceModel(id)?.name ?? id;
}

export function isValidChoice(choice: unknown): choice is VirtualIphoneChoice {
  if (!choice || typeof choice !== "object") {
    return false;
  }
  const c = choice as Partial<VirtualIphoneChoice>;
  return catalog.pairings.some((p) => p.device === c.device && p.ios === c.ios);
}
