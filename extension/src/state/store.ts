// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// An observable snapshot of what the UI needs: devices, the active device,
// the open project, the selected target/configuration and the run session.
// ide.md §5.2 (state/store.ts).

import * as vscode from "vscode";
import { Emitter } from "../util/disposables";
import type {
  DeviceSummary,
  ProjectInfoResult,
  SessionState,
} from "../generated/protocol";

export type ProjectKind = ProjectInfoResult["stack"];

export interface RunStatus {
  state: SessionState | "idle";
  sessionId?: string;
  isDebug: boolean;
  bundleId?: string;
}

export interface BuildStatus {
  phase: "idle" | "building" | "ok" | "error";
  durationMs?: number;
  errorCount?: number;
}

export interface StoreSnapshot {
  devices: DeviceSummary[];
  activeDeviceUdid?: string;
  project?: ProjectInfoResult;
  selectedTarget?: string;
  selectedConfiguration: string;
  run: RunStatus;
  build: BuildStatus;
}

const WORKSPACE_DEVICE_KEY = "russet.activeDeviceUdid";
const WORKSPACE_TARGET_KEY = "russet.selectedTarget";

export class Store implements vscode.Disposable {
  private snapshot: StoreSnapshot = {
    devices: [],
    selectedConfiguration: "Debug",
    run: { state: "idle", isDebug: false },
    build: { phase: "idle" },
  };

  private readonly _onDidChange = new Emitter<StoreSnapshot>();
  readonly onDidChange = this._onDidChange.event;

  private readonly _onDidChangeActiveDevice = new Emitter<DeviceSummary | undefined>();
  readonly onDidChangeActiveDevice = this._onDidChangeActiveDevice.event;

  constructor(private readonly memento: vscode.Memento) {
    this.snapshot.activeDeviceUdid = memento.get<string>(WORKSPACE_DEVICE_KEY);
    this.snapshot.selectedTarget = memento.get<string>(WORKSPACE_TARGET_KEY);
  }

  get state(): Readonly<StoreSnapshot> {
    return this.snapshot;
  }

  get activeDevice(): DeviceSummary | undefined {
    const udid = this.snapshot.activeDeviceUdid;
    return this.snapshot.devices.find((d) => d.udid === udid);
  }

  private emit(): void {
    this._onDidChange.fire(this.snapshot);
  }

  setDevices(devices: DeviceSummary[]): void {
    this.snapshot.devices = devices;
    // If the active device vanished, or none is chosen, fall back to the first ready one.
    const active = this.activeDevice;
    if (!active) {
      const pick = devices.find((d) => d.state === "ready") ?? devices[0];
      this.setActiveDevice(pick?.udid, false);
    }
    this.emit();
    this._onDidChangeActiveDevice.fire(this.activeDevice);
  }

  upsertDevice(device: DeviceSummary, removed = false): void {
    const next = this.snapshot.devices.filter((d) => d.udid !== device.udid);
    if (!removed) {
      next.push(device);
    }
    this.setDevices(next);
  }

  setActiveDevice(udid: string | undefined, persist = true): void {
    this.snapshot.activeDeviceUdid = udid;
    if (persist) {
      void this.memento.update(WORKSPACE_DEVICE_KEY, udid);
      this.emit();
      this._onDidChangeActiveDevice.fire(this.activeDevice);
    }
  }

  setProject(project: ProjectInfoResult | undefined): void {
    this.snapshot.project = project;
    if (project) {
      if (!this.snapshot.selectedTarget || !project.targets.some((t) => t.name === this.snapshot.selectedTarget)) {
        this.snapshot.selectedTarget = project.defaultTarget ?? project.targets[0]?.name;
      }
    }
    this.emit();
  }

  setSelectedTarget(target: string | undefined): void {
    this.snapshot.selectedTarget = target;
    void this.memento.update(WORKSPACE_TARGET_KEY, target);
    this.emit();
  }

  setSelectedConfiguration(configuration: string): void {
    this.snapshot.selectedConfiguration = configuration;
    this.emit();
  }

  setRun(run: RunStatus): void {
    this.snapshot.run = run;
    this.emit();
  }

  setBuild(build: BuildStatus): void {
    this.snapshot.build = build;
    this.emit();
  }

  /** Whether the current stack supports hot reload while a session runs (ide.md §5.7). */
  get canHotReload(): boolean {
    const running = this.snapshot.run.state === "running" || this.snapshot.run.state === "ready";
    const stack = this.snapshot.project?.stack;
    return running && (stack === "flutter" || stack === "swiftpm");
  }

  dispose(): void {
    this._onDidChange.dispose();
    this._onDidChangeActiveDevice.dispose();
  }
}
