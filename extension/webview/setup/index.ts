// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The guided setup page (W19): one card per setup step with status, progress,
// log tail and Run / Retry / Repair / Cancel. The host owns all state
// (src/setup/model.ts) and sends a snapshot; this page renders it and posts
// the user's intent back. Steps are re-rendered only when their own data
// changes, so focus and open pickers survive progress updates.

import { vscodeApi } from "../shared/vscode";
import { addStyles, announcer, h } from "../shared/dom";
import baseCss from "../shared/base.css?inline";
import pageCss from "./setup.css?inline";

type StepState = "done" | "needed" | "running" | "failed" | "unavailable" | "unknown";
type StepAction = "run" | "retry" | "repair" | "cancel" | "recheck" | "signIn" | "addKey" | "pickDevice";

interface ViewStep {
  id: string;
  title: string;
  detail?: string;
  state: StepState;
  source: "runner" | "daemon";
  progress?: { message: string; fraction?: number };
  log: string[];
  error?: { code: string; message: string; retryable: boolean };
  actions: StepAction[];
  blockedReason?: string;
}

interface Snapshot {
  runnerAvailable: boolean;
  runnerError?: string;
  loading: boolean;
  daemonConnected: boolean;
  steps: ViewStep[];
  doneCount: number;
  totalCount: number;
  runningStep?: string;
  runningAll: boolean;
  xcode: { candidates: Array<{ path: string; fileName: string; version?: string; sizeBytes: number }>; selected?: string };
  virtual: { device: string; ios: string };
  account: { label?: string; signInEnabled: boolean };
}

interface Catalog {
  devices: Array<{ id: string; name: string }>;
  pairings: Array<{ device: string; ios: string; build: string; linuxTested: boolean }>;
  defaultChoice: { device: string; ios: string };
}

interface PersistedState {
  openLogs: string[];
}

const vscode = vscodeApi();
addStyles(baseCss, pageCss);
const announce = announcer();
const persisted: PersistedState = vscode.getState<PersistedState>() ?? { openLogs: [] };
const openLogs = new Set(persisted.openLogs);

let catalog: Catalog = { devices: [], pairings: [], defaultChoice: { device: "", ios: "" } };
let snapshot: Snapshot | undefined;
const rendered = new Map<string, { sig: string; el: HTMLElement }>();

const STATE_LABEL: Record<StepState, string> = {
  done: "Done",
  needed: "To do",
  running: "Running",
  failed: "Failed",
  unavailable: "Not available",
  unknown: "Unknown",
};
const STATE_ICON: Record<StepState, string> = {
  done: "✓",
  needed: "○",
  running: "◌",
  failed: "✕",
  unavailable: "!",
  unknown: "?",
};
/** Actions that move setup forward get the primary button; the rest are secondary. */
const PRIMARY: ReadonlySet<StepAction> = new Set(["run", "retry", "signIn"]);

const ACTION_LABEL: Record<StepAction, string> = {
  run: "Set up",
  retry: "Retry",
  repair: "Repair",
  cancel: "Cancel",
  recheck: "Check again",
  signIn: "Sign in with Apple ID…",
  addKey: "Add App Store Connect API key…",
  pickDevice: "Choose iPhone…",
};

const post = (msg: unknown): void => vscode.postMessage(msg);

// --- Page skeleton ------------------------------------------------------------

const summaryText = h("p", { id: "summary", class: "muted" });
const overall = h("progress", { id: "overall", max: 1, value: 0, "aria-label": "Overall setup progress" });
const runAllBtn = h("button", { id: "run-all", "data-focus-key": "run-all", onclick: () => post({ kind: "runAll" }) }, "Set up remaining steps");
const cancelAllBtn = h("button", { id: "cancel-all", class: "secondary", "data-focus-key": "cancel-all", onclick: () => post({ kind: "cancel" }) }, "Stop");
const refreshBtn = h("button", { class: "secondary", "data-focus-key": "refresh", onclick: () => post({ kind: "refresh" }) }, "Check again");
const notices = h("div", { id: "notices" });
const list = h("ol", { class: "steps", "aria-label": "Setup steps" });

document.getElementById("root")?.append(
  h(
    "main",
    { "aria-labelledby": "page-title" },
    h(
      "header",
      { class: "page-head" },
      h("h1", { id: "page-title" }, "Set up Russet"),
      h("p", { class: "muted" }, "Build, run and debug iOS apps on this PC, on your iPhone and on a virtual iPhone."),
      h("div", { class: "summary" }, summaryText, overall),
      h("div", { class: "row" }, runAllBtn, cancelAllBtn, refreshBtn,
        h("button", { class: "link", onclick: () => post({ kind: "openWalkthrough" }) }, "Open the Getting Started walkthrough")),
    ),
    notices,
    list,
  ),
);

// --- Rendering ----------------------------------------------------------------

function render(s: Snapshot): void {
  const runnerSteps = s.steps;
  summaryText.textContent = s.loading && runnerSteps.length === 0
    ? "Checking this PC…"
    : `${s.doneCount} of ${s.totalCount} steps done${s.runningStep ? ` · running: ${titleOf(s, s.runningStep)}` : ""}`;
  overall.max = Math.max(1, s.totalCount);
  overall.value = s.doneCount;
  overall.setAttribute("aria-valuetext", `${s.doneCount} of ${s.totalCount} steps done`);

  runAllBtn.hidden = !s.runnerAvailable || s.runningAll || s.doneCount === s.totalCount;
  runAllBtn.disabled = !!s.runningStep || s.doneCount === s.totalCount;
  cancelAllBtn.hidden = !s.runningAll && !s.runningStep;
  refreshBtn.disabled = s.loading;

  const n: HTMLElement[] = [];
  if (!s.runnerAvailable) {
    n.push(h("div", { class: "alert info", role: "note" },
      "Setting up this PC (WSL, the Russet backend, Apple Devices and the virtual iPhone) runs in the installed Russet IDE on Windows. The engine-backed steps are shown below."));
  }
  if (s.runnerError) {
    n.push(h("div", { class: "alert", role: "alert" }, `The setup runner could not report its status: ${s.runnerError}`));
  }
  notices.replaceChildren(...n);

  const seen = new Set<string>();
  let previous: HTMLElement | undefined;
  for (const step of s.steps) {
    seen.add(step.id);
    const sig = JSON.stringify([step, extrasSig(s, step)]);
    let entry = rendered.get(step.id);
    if (!entry || entry.sig !== sig) {
      const el = renderStep(s, step);
      if (entry) {
        const active = document.activeElement as HTMLElement | null;
        const key = active && entry.el.contains(active) ? active.dataset.focusKey : undefined;
        entry.el.replaceWith(el);
        if (key) {
          el.querySelector<HTMLElement>(`[data-focus-key="${CSS.escape(key)}"]`)?.focus();
        }
      }
      entry = { sig, el };
      rendered.set(step.id, entry);
    }
    // Keep DOM order equal to snapshot order.
    const expectedNext: Element | null = previous ? previous.nextElementSibling : list.firstElementChild;
    if (expectedNext !== entry.el) {
      list.insertBefore(entry.el, expectedNext);
    }
    previous = entry.el;
  }
  for (const [id, entry] of rendered) {
    if (!seen.has(id)) {
      entry.el.remove();
      rendered.delete(id);
    }
  }
}

function titleOf(s: Snapshot, id: string): string {
  return s.steps.find((x) => x.id === id)?.title ?? id;
}

/** Page-level data a step's extras depend on. */
function extrasSig(s: Snapshot, step: ViewStep): unknown {
  switch (step.id) {
    case "xcode-sdk":
      return s.xcode;
    case "virtual-iphone":
      return s.virtual;
    case "account":
      return s.account;
    default:
      return null;
  }
}

function renderStep(s: Snapshot, step: ViewStep): HTMLElement {
  const titleId = `title-${step.id}`;
  const li = h("li", { class: `step card state-${step.state}`, id: `step-${step.id}`, "aria-labelledby": titleId });
  li.append(
    h(
      "div",
      { class: "step-head" },
      h("span", { class: `icon ${step.state === "running" ? "spin" : ""}`, "aria-hidden": "true" }, STATE_ICON[step.state]),
      h("h2", { id: titleId }, step.title),
      h("span", { class: `badge ${step.state}` }, STATE_LABEL[step.state]),
    ),
  );
  if (step.detail) {
    li.append(h("p", { class: "detail" }, step.detail));
  }
  const extras = renderExtras(s, step);
  if (extras) {
    li.append(extras);
  }
  if (step.progress) {
    const pct = step.progress.fraction;
    li.append(
      h(
        "div",
        { class: "progress" },
        pct === undefined
          ? h("progress", { "aria-label": `${step.title} progress` })
          : h("progress", { max: 1, value: pct, "aria-label": `${step.title} progress`, "aria-valuetext": `${Math.round(pct * 100)} percent` }),
        h("p", { class: "muted progress-message", "aria-live": "polite" },
          pct === undefined ? step.progress.message : `${Math.round(pct * 100)}% · ${step.progress.message}`),
      ),
    );
  }
  if (step.error) {
    li.append(
      h("div", { class: "alert", role: "alert" },
        h("strong", {}, "This step did not finish. "), step.error.message,
        h("span", { class: "code muted" }, ` (${step.error.code})`)),
    );
  }
  if (step.log.length > 0) {
    const details = h("details", { class: "log" });
    details.open = openLogs.has(step.id);
    details.addEventListener("toggle", () => {
      if (details.open) {
        openLogs.add(step.id);
      } else {
        openLogs.delete(step.id);
      }
      vscode.setState<PersistedState>({ openLogs: [...openLogs] });
    });
    const pre = h("pre", { role: "log", tabindex: 0, "aria-label": `${step.title} log` }, step.log.join("\n"));
    details.append(h("summary", { "data-focus-key": `log:${step.id}` }, `Log (last ${step.log.length} lines)`), pre);
    li.append(details);
    if (details.open) {
      queueMicrotask(() => {
        pre.scrollTop = pre.scrollHeight;
      });
    }
  }
  const actions = renderActions(step);
  if (actions) {
    li.append(actions);
  }
  return li;
}

function renderActions(step: ViewStep): HTMLElement | undefined {
  if (step.actions.length === 0) {
    return undefined;
  }
  const reasonId = `reason-${step.id}`;
  const row = h("div", { class: "row actions" });
  step.actions.forEach((action, i) => {
    const blocked = !!step.blockedReason && action !== "cancel" && action !== "recheck";
    row.append(
      h(
        "button",
        {
          class: i === 0 && PRIMARY.has(action) ? "" : "secondary",
          "data-focus-key": `${step.id}:${action}`,
          "aria-describedby": blocked ? reasonId : `title-${step.id}`,
          disabled: blocked,
          onclick: () => onAction(step, action),
        },
        ACTION_LABEL[action],
      ),
    );
  });
  if (step.blockedReason) {
    row.append(h("span", { id: reasonId, class: "muted" }, step.blockedReason));
  }
  return row;
}

function onAction(step: ViewStep, action: StepAction): void {
  switch (action) {
    case "run":
    case "retry":
    case "repair":
      announce(`${ACTION_LABEL[action]}: ${step.title}`);
      post({ kind: "run", id: step.id, mode: action });
      break;
    case "cancel":
      post({ kind: "cancel" });
      break;
    case "recheck":
      post({ kind: "refresh" });
      break;
    case "signIn":
      post({ kind: "signIn" });
      break;
    case "addKey":
      post({ kind: "addKey" });
      break;
    case "pickDevice":
      post({ kind: "pickDevice" });
      break;
  }
}

function renderExtras(s: Snapshot, step: ViewStep): HTMLElement | undefined {
  switch (step.id) {
    case "wsl":
      if (step.state === "done") {
        return undefined;
      }
      return h(
        "div",
        { class: "extras" },
        h("p", {}, "Install WSL from an administrator terminal (Windows may restart), then press Check again:"),
        h("div", { class: "row" },
          h("code", { class: "cmd" }, "wsl --install --no-distribution"),
          h("button", { class: "secondary", "data-focus-key": "wsl:copy", onclick: () => post({ kind: "copyWslCommand" }) }, "Copy command")),
      );
    case "apple-devices":
      if (step.state === "done") {
        return undefined;
      }
      return h(
        "div",
        { class: "extras row" },
        h("button", { class: "secondary", "data-focus-key": "apple-devices:store", onclick: () => post({ kind: "openExternal", target: "apple-devices" }) },
          "Get Apple Devices from the Microsoft Store"),
      );
    case "xcode-sdk":
      return renderXcode(s, step);
    case "virtual-iphone":
      return renderVirtual(s, step);
    case "account":
      return s.account.label ? h("p", { class: "muted" }, `Default account: ${s.account.label}`) : undefined;
    default:
      return undefined;
  }
}

function renderXcode(s: Snapshot, step: ViewStep): HTMLElement {
  const locked = step.state === "running";
  const fs = h("fieldset", { class: "extras" }, h("legend", {}, "Xcode archive"));
  const all = [...s.xcode.candidates];
  if (s.xcode.selected && !all.some((c) => c.path === s.xcode.selected)) {
    all.push({ path: s.xcode.selected, fileName: baseName(s.xcode.selected), sizeBytes: -1 });
  }
  if (all.length === 0) {
    fs.append(h("p", { class: "muted" }, "No Xcode_*.xip was found in your Downloads folder."));
  }
  all.forEach((c, i) => {
    const id = `xip-${i}`;
    const input = h("input", {
      type: "radio",
      name: "xip",
      id,
      value: c.path,
      "data-focus-key": `xip:${c.path}`,
      checked: c.path === s.xcode.selected,
      disabled: locked,
      onchange: () => post({ kind: "selectXip", path: c.path }),
    });
    const where = c.sizeBytes >= 0 ? `in Downloads · ${formatBytes(c.sizeBytes)}` : "chosen with Browse";
    fs.append(
      h("div", { class: "radio" }, input,
        h("label", { for: id, title: c.path }, `${c.version ? `Xcode ${c.version}` : c.fileName} `, h("span", { class: "muted" }, `(${c.fileName}, ${where})`))),
    );
  });
  fs.append(
    h("div", { class: "row" },
      h("button", { class: "secondary", "data-focus-key": "xip:browse", disabled: locked, onclick: () => post({ kind: "browseXip" }) }, "Browse…"),
      h("button", { class: "link", onclick: () => post({ kind: "openExternal", target: "xcode-download" }) }, "Download Xcode from Apple")),
    h("p", { class: "muted small" }, "Russet reads the SDK from the archive you downloaded with your Apple ID; it never redistributes Xcode. After you sign in, Russet will offer to download Xcode for you."),
  );
  return fs;
}

function renderVirtual(s: Snapshot, step: ViewStep): HTMLElement {
  const locked = step.state === "running";
  const choice = s.virtual;
  const deviceSel = h("select", {
    id: "virtual-device",
    "data-focus-key": "virtual-device",
    disabled: locked,
    onchange: (ev: Event) => {
      const device = (ev.target as HTMLSelectElement).value;
      const first = catalog.pairings.find((p) => p.device === device && p.linuxTested) ??
        catalog.pairings.find((p) => p.device === device);
      if (first) {
        post({ kind: "selectVirtual", device, ios: first.ios });
      }
    },
  });
  for (const d of catalog.devices) {
    deviceSel.append(h("option", { value: d.id, selected: d.id === choice.device }, `${d.name} (${d.id})`));
  }
  const iosSel = h("select", {
    id: "virtual-ios",
    "data-focus-key": "virtual-ios",
    disabled: locked,
    onchange: (ev: Event) => post({ kind: "selectVirtual", device: choice.device, ios: (ev.target as HTMLSelectElement).value }),
  });
  for (const p of catalog.pairings.filter((x) => x.device === choice.device)) {
    const isDefault = p.device === catalog.defaultChoice.device && p.ios === catalog.defaultChoice.ios;
    iosSel.append(
      h("option", { value: p.ios, selected: p.ios === choice.ios },
        `iOS ${p.ios} (${p.build})${p.linuxTested ? " · tested" : " · untested"}${isDefault ? " · default" : ""}`),
    );
  }
  const pairing = catalog.pairings.find((p) => p.device === choice.device && p.ios === choice.ios);
  return h(
    "div",
    { class: "extras virtual" },
    h("div", { class: "field" }, h("label", { for: "virtual-device" }, "Device"), deviceSel),
    h("div", { class: "field" }, h("label", { for: "virtual-ios" }, "iOS version"), iosSel),
    pairing && !pairing.linuxTested
      ? h("p", { class: "muted small" }, "This pairing has not been verified end to end on Linux yet; creating it may fail.")
      : h("p", { class: "muted small" }, "Creating the virtual iPhone takes a few hours. You can keep working; the step resumes if it is interrupted."),
  );
}

function baseName(p: string): string {
  const parts = p.split(/[\\/]/);
  return parts[parts.length - 1] ?? p;
}

function formatBytes(n: number): string {
  if (n >= 1024 ** 3) {
    return `${(n / 1024 ** 3).toFixed(1)} GB`;
  }
  return `${Math.max(1, Math.round(n / 1024 ** 2))} MB`;
}

// --- Messages -----------------------------------------------------------------

window.addEventListener("message", (event: MessageEvent) => {
  const msg = event.data as { kind?: string; snapshot?: Snapshot; catalog?: Catalog; id?: string; text?: string };
  switch (msg.kind) {
    case "catalog":
      if (msg.catalog) {
        catalog = msg.catalog;
        rendered.clear();
        list.replaceChildren();
        if (snapshot) {
          render(snapshot);
        }
      }
      break;
    case "state":
      if (msg.snapshot) {
        const before = snapshot;
        snapshot = msg.snapshot;
        render(snapshot);
        announceTransitions(before, snapshot);
      }
      break;
    case "focus": {
      const el = msg.id ? document.getElementById(`step-${msg.id}`) : null;
      if (el) {
        el.scrollIntoView({ block: "center" });
        el.querySelector<HTMLElement>("button:not(:disabled), select, input")?.focus();
      }
      break;
    }
    case "announce":
      if (msg.text) {
        announce(msg.text);
      }
      break;
    default:
      break;
  }
});

function announceTransitions(before: Snapshot | undefined, after: Snapshot): void {
  if (!before) {
    return;
  }
  for (const step of after.steps) {
    const prev = before.steps.find((x) => x.id === step.id);
    if (prev && prev.state === "running" && step.state !== "running") {
      announce(`${step.title}: ${STATE_LABEL[step.state]}`);
    }
  }
}

post({ kind: "ready" });
