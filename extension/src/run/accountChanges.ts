// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Run and Debug stop with perm.approvalRequired ("provisioning needs N Apple-side change(s)") when the app or
// the iPhone needs changes in the Apple developer account first: registering the iPhone or the app's bundle ID,
// or creating a certificate or a development profile. Such changes need a person's approval (ARCHITECTURE
// §11.3). Russet lists them, and once the user agrees in its own dialog makes them (provision.plan,
// provision.apply, approval.grant), so the run can go on. No command id can grant an approval.

import * as vscode from "vscode";
import type { RussetClient } from "../daemon/client";
import { errorDataOf, showDaemonError } from "../daemon/errors";
import type { ApprovalRequiredDetails, PlanMutation } from "../generated/protocol";

/** Run's or Debug's "provisioning needs … Apple-side change(s)", not an approval request that has an id. */
export function needsAccountChanges(err: unknown): boolean {
  const data = errorDataOf(err);
  return data?.russetCode === "perm.approvalRequired" && !approvalIdOf(err);
}

function approvalIdOf(err: unknown): string | undefined {
  const details = errorDataOf(err)?.details as Partial<ApprovalRequiredDetails> | null | undefined;
  return typeof details?.approvalId === "string" ? details.approvalId : undefined;
}

/** The dialog's text: what Russet will change. */
export function describeChanges(mutations: readonly PlanMutation[]): { message: string; detail: string } {
  const n = mutations.length;
  return {
    message: `Running on this iPhone needs ${n} change${n === 1 ? "" : "s"} in your Apple developer account`,
    detail: [...mutations.map((m) => `• ${m.summary}`), "", "Russet makes only these changes, then runs the app."].join("\n"),
  };
}

/**
 * After Run or Debug stopped for account changes: lists them and, if the user agrees, makes them. "made": run
 * again; "declined": the user said no, or the changes failed (already shown).
 */
export async function makeAccountChanges(
  client: Pick<RussetClient, "call">,
  project: string,
  udid: string | undefined,
): Promise<"made" | "declined"> {
  try {
    const device = udid ?? (await onlyDevice(client));
    if (!device) {
      void vscode.window.showErrorMessage("Connect your iPhone and choose it in the device picker, then run again.");
      return "declined";
    }
    const plan = await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Notification, title: "Checking your Apple developer account" },
      () => client.call("provision.plan", { project, distribution: "development", devices: [device] }),
    );
    if (plan.mutations.length === 0) return "made";
    const { message, detail } = describeChanges(plan.mutations);
    const pick = await vscode.window.showWarningMessage(message, { modal: true, detail }, "Make the Changes");
    if (pick !== "Make the Changes") return "declined";
    await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Notification, title: "Changing your Apple developer account" },
      () => apply(client, plan.planId),
    );
    return "made";
  } catch (err) {
    await showDaemonError(err, "Your Apple developer account was not changed");
    return "declined";
  }
}

/** Applies a plan: the daemon asks for a person's approval first, which the user just gave in the dialog. */
async function apply(client: Pick<RussetClient, "call">, planId: string): Promise<void> {
  try {
    await client.call("provision.apply", { planId });
  } catch (err) {
    const approvalId = approvalIdOf(err);
    if (errorDataOf(err)?.russetCode !== "perm.approvalRequired" || !approvalId) throw err;
    await client.call("approval.grant", { approvalId, scope: "once" });
    await client.call("provision.apply", { planId, approval: approvalId });
  }
}

/** The one connected iPhone, as Run picks it when none is chosen. */
async function onlyDevice(client: Pick<RussetClient, "call">): Promise<string | undefined> {
  const { devices } = await client.call("device.list", {});
  return devices.length === 1 ? devices[0].udid : undefined;
}
