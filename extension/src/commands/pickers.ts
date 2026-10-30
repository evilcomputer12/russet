// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Quick-pick run-destination, target and configuration selectors (EXT-03).

import * as vscode from "vscode";
import type { Store } from "../state/store";

export async function pickDevice(store: Store): Promise<void> {
  const devices = store.state.devices;
  if (devices.length === 0) {
    void vscode.window.showInformationMessage("No iPhones are connected.");
    return;
  }
  const items = devices.map((d) => ({
    label: `$(device-mobile) ${d.marketingName ?? d.name}`,
    description: `iOS ${d.osVersion} · ${d.transport}`,
    detail: d.state !== "ready" ? `State: ${d.state}` : undefined,
    udid: d.udid,
  }));
  const choice = await vscode.window.showQuickPick(items, {
    title: "Select Run Destination",
    placeHolder: "Choose the iPhone to run on",
  });
  if (choice) {
    store.setActiveDevice(choice.udid);
  }
}

export async function pickTarget(store: Store): Promise<void> {
  const project = store.state.project;
  if (!project || project.targets.length === 0) {
    void vscode.window.showInformationMessage("No project targets are available.");
    return;
  }
  const choice = await vscode.window.showQuickPick(
    project.targets.map((t) => ({ label: t.name, description: t.kind, detail: t.bundleId })),
    { title: "Select Target", placeHolder: "Choose the target to build and run" },
  );
  if (choice) {
    store.setSelectedTarget(choice.label);
  }
}

export async function pickConfiguration(store: Store): Promise<void> {
  const options = store.state.project?.configurations.map((c) => capitalize(c)) ?? ["Debug", "Release"];
  const choice = await vscode.window.showQuickPick([...new Set(options)], {
    title: "Select Configuration",
    placeHolder: "Choose the build configuration",
  });
  if (choice) {
    store.setSelectedConfiguration(choice);
  }
}

function capitalize(s: string): string {
  return s.length ? s[0].toUpperCase() + s.slice(1) : s;
}
