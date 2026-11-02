// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Run destinations, Xcode style: physical iPhones from `device.list` and
// virtual iPhones in one picker. Virtual entries come from the setup runner's
// `virtual-iphone` step today; once the daemon reports devices with
// `transport: "virtual"` (contract request in docs/windows/W19-CONTRACT-REQUESTS.md)
// those slot in unchanged and become runnable.

import * as vscode from "vscode";
import type { DeviceSummary } from "../generated/protocol";
import type { Store } from "../state/store";
import { Emitter } from "../util/disposables";
import { deviceModel, deviceName, type VirtualIphoneChoice } from "../setup/virtualCatalog";

export type DestinationKind = "physical" | "virtual";

export interface RunDestination {
  /** A UDID for daemon devices, `virtual:<product>:<ios>` for a runner-created VM. */
  id: string;
  kind: DestinationKind;
  name: string;
  productType?: string;
  osVersion?: string;
  transport: "usb" | "wifi" | "virtual";
  /** Ready to run on now. */
  ready: boolean;
  /** Short state for the picker, e.g. "Ready", "Needs trust", "Not running". */
  stateLabel: string;
  device?: DeviceSummary;
  screen?: [number, number];
}

/** What the setup knows about the runner-created virtual iPhone. */
export interface VirtualSource {
  created: boolean;
  /** Its VM runs (the runner's start and stop, and its periodic checks). */
  running?: boolean;
  choice: VirtualIphoneChoice;
}

const STATE_LABEL: Record<string, string> = {
  ready: "Ready",
  attached: "Connecting",
  "needs-trust": "Needs trust",
  "trust-pending": "Waiting for Trust",
  unavailable: "Unavailable",
};

export function isVirtualDevice(d: DeviceSummary): boolean {
  return (d.transport as string) === "virtual";
}

export function fromDevice(d: DeviceSummary): RunDestination {
  const virtual = isVirtualDevice(d);
  return {
    id: d.udid,
    kind: virtual ? "virtual" : "physical",
    name: d.marketingName ?? (virtual ? deviceName(d.productType) : d.name),
    productType: d.productType,
    osVersion: d.osVersion,
    transport: virtual ? "virtual" : d.transport,
    ready: d.state === "ready",
    stateLabel: STATE_LABEL[d.state] ?? d.state,
    device: d,
    ...(virtual && deviceModel(d.productType) ? { screen: deviceModel(d.productType)!.screen } : {}),
  };
}

export function virtualId(choice: VirtualIphoneChoice): string {
  return `virtual:${choice.device}:${choice.ios}`;
}

/** Physical devices first (ready first), then virtual ones. */
export function mergeDestinations(devices: readonly DeviceSummary[], virtual: VirtualSource | undefined): RunDestination[] {
  const fromDaemon = devices.map(fromDevice);
  const out = [...fromDaemon];
  // A runner-created VM the daemon does not report yet: listed, not runnable.
  if (virtual?.created && !fromDaemon.some((d) => d.kind === "virtual" && d.productType === virtual.choice.device)) {
    const model = deviceModel(virtual.choice.device);
    out.push({
      id: virtualId(virtual.choice),
      kind: "virtual",
      name: model?.name ?? virtual.choice.device,
      productType: virtual.choice.device,
      osVersion: virtual.choice.ios,
      transport: "virtual",
      ready: false,
      stateLabel: virtual.running ? "Running" : "Not running",
      ...(model ? { screen: model.screen } : {}),
    });
  }
  return out.sort((a, b) => rank(a) - rank(b));
}

function rank(d: RunDestination): number {
  return (d.kind === "virtual" ? 2 : 0) + (d.ready ? 0 : 1);
}

const SELECTED_KEY = "russet.selectedDestination";

export class DestinationModel implements vscode.Disposable {
  private readonly _onDidChange = new Emitter<RunDestination | undefined>();
  readonly onDidChange = this._onDidChange.event;
  private readonly disposables: vscode.Disposable[] = [];
  private selectedId?: string;

  constructor(
    private readonly store: Store,
    private readonly memento: vscode.Memento,
    private readonly virtualSource: () => VirtualSource | undefined,
  ) {
    this.selectedId = memento.get<string>(SELECTED_KEY);
    this.disposables.push(store.onDidChange(() => this._onDidChange.fire(this.selected)));
  }

  /** Re-read the virtual source (after setup changes). */
  refresh(): void {
    this._onDidChange.fire(this.selected);
  }

  get all(): RunDestination[] {
    return mergeDestinations(this.store.state.devices, this.virtualSource());
  }

  /** The chosen destination: the user's pick, else the store's active device. */
  get selected(): RunDestination | undefined {
    const all = this.all;
    if (this.selectedId?.startsWith("virtual:")) {
      const v = all.find((d) => d.id === this.selectedId) ??
        all.find((d) => d.kind === "virtual" && this.selectedId?.startsWith(`virtual:${d.productType}:`));
      if (v) {
        return v;
      }
    }
    const active = this.store.activeDevice;
    return active ? fromDevice(active) : undefined;
  }

  select(id: string): void {
    const dest = this.all.find((d) => d.id === id);
    if (!dest) {
      return;
    }
    this.selectedId = id;
    void this.memento.update(SELECTED_KEY, id);
    if (dest.device) {
      this.store.setActiveDevice(dest.device.udid);
    } else {
      this._onDidChange.fire(this.selected);
    }
  }

  /** Why Run/Debug cannot use the selected destination yet, if it cannot. */
  runBlocker(): string | undefined {
    const d = this.selected;
    if (d?.kind === "virtual" && !d.device && d.stateLabel !== "Running") {
      return `${d.name} (virtual) is not running: start it from the run destination picker or the Devices view first.`;
    }
    return undefined;
  }

  dispose(): void {
    this._onDidChange.dispose();
    for (const d of this.disposables.splice(0)) {
      d.dispose();
    }
  }
}

interface DestinationItem extends vscode.QuickPickItem {
  destinationId?: string;
  createVirtual?: boolean;
  /** A command that starts or stops the runner-created virtual iPhone. */
  command?: string;
}

/** The Xcode-style run destination picker. */
export async function pickDestination(model: DestinationModel, canCreateVirtual: boolean): Promise<void> {
  const all = model.all;
  const selected = model.selected?.id;
  const items: DestinationItem[] = [];
  const physical = all.filter((d) => d.kind === "physical");
  const virtual = all.filter((d) => d.kind === "virtual");
  items.push({ label: "iPhones", kind: vscode.QuickPickItemKind.Separator });
  if (physical.length === 0) {
    items.push({ label: "$(info) No iPhone connected", description: "Connect one with USB and tap Trust", alwaysShow: true });
  }
  for (const d of physical) {
    items.push(itemFor(d, selected));
  }
  items.push({ label: "Virtual iPhones", kind: vscode.QuickPickItemKind.Separator });
  for (const d of virtual) {
    items.push(itemFor(d, selected));
    // A runner-created VM (not a daemon device) is started and stopped through the runner.
    if (canCreateVirtual && d.id.startsWith("virtual:")) {
      items.push(
        d.stateLabel === "Running"
          ? { label: `$(debug-stop) Stop ${d.name}`, description: "Shut the virtual iPhone down", command: "russet.virtual.stop" }
          : { label: `$(play) Start ${d.name}`, description: "Opens its window; several minutes to the lock screen", command: "russet.virtual.start" },
      );
    }
  }
  if (canCreateVirtual && virtual.length === 0) {
    items.push({ label: "$(add) Create a Virtual iPhone…", description: "Guided setup", createVirtual: true });
  }
  const choice = await vscode.window.showQuickPick(items, {
    title: "Select Run Destination",
    placeHolder: "Choose where to run and debug",
    matchOnDescription: true,
  });
  if (!choice) {
    return;
  }
  if (choice.createVirtual) {
    await vscode.commands.executeCommand("russet.openSetup", "virtual-iphone");
    return;
  }
  if (choice.command) {
    await vscode.commands.executeCommand(choice.command);
    return;
  }
  if (choice.destinationId) {
    model.select(choice.destinationId);
  }
}

function itemFor(d: RunDestination, selected: string | undefined): DestinationItem {
  const icon = d.kind === "virtual" ? "$(vm)" : "$(device-mobile)";
  const transport = d.transport === "usb" ? "USB" : d.transport === "wifi" ? "Wi-Fi" : "Virtual";
  return {
    label: `${icon} ${d.name}`,
    description: [d.osVersion ? `iOS ${d.osVersion}` : undefined, transport, d.ready ? undefined : d.stateLabel]
      .filter(Boolean)
      .join(" · "),
    ...(d.id === selected ? { picked: true, detail: "$(check) Current run destination" } : {}),
    destinationId: d.id,
  };
}
