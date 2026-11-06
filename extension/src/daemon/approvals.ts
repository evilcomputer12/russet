// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Approvals for AI agents (ARCHITECTURE §11.3): an agent's call that changes the Apple developer account or
// publishes stops with perm.approvalRequired, and the daemon announces it (approval.requested). Russet asks the
// user in its own dialog, which no command id can answer; Approve grants it (approval.grant) and the agent
// retries with the approval's id. The IDE's own requests (Run's account changes) are asked where they arise
// (run/accountChanges.ts). A dialog closed without an answer leaves the request pending: it expires.

import * as vscode from "vscode";
import type { RussetClient } from "./client";
import { messageOf } from "./errors";
import type { Approval } from "../generated/protocol";

export function registerApprovals(client: Pick<RussetClient, "call" | "onEvent">): vscode.Disposable {
  return client.onEvent("approval.requested", ({ approval }) => {
    if (approval.requestedBy.kind !== "ide") void ask(client, approval);
  });
}

/** The dialog's text for a request. */
export function describeApproval(approval: Approval, now = new Date()): { message: string; detail: string } {
  const by = approval.requestedBy;
  const who = by.kind === "mcp" ? `The AI agent "${by.name}"` : `"${by.name}" (${by.kind})`;
  const minutes = Math.max(0, Math.round((Date.parse(approval.expiresAt) - now.getTime()) / 60_000));
  return {
    message: `${who} asks for your approval: ${approval.summary}`,
    detail: [
      approval.detail,
      approval.tier >= 3
        ? "This publishes your app or its metadata."
        : "This changes your Apple developer account (it only adds: devices, bundle IDs, certificates, profiles).",
      `Approve it only if you asked the agent for this. The request expires in ${minutes} min.`,
    ].filter((line): line is string => !!line).join("\n\n"),
  };
}

async function ask(client: Pick<RussetClient, "call">, approval: Approval): Promise<void> {
  const { message, detail } = describeApproval(approval);
  const pick = await vscode.window.showWarningMessage(message, { modal: true, detail }, "Approve", "Deny");
  if (!pick) return;
  const approvalId = approval.approvalId;
  try {
    if (pick === "Approve") await client.call("approval.grant", { approvalId });
    else await client.call("approval.deny", { approvalId, reason: "denied in Russet IDE" });
  } catch (err) {
    // Answered meanwhile (another window, `russet approve`) or expired: nothing to say if it is no longer pending.
    const current = await client.call("approval.get", { approvalId }).catch(() => undefined);
    if (current && current.state !== "pending") {
      if (current.state === "expired") void vscode.window.showWarningMessage("The request expired before your answer; the agent can ask again.");
      return;
    }
    void vscode.window.showErrorMessage(`The request was not ${pick === "Approve" ? "approved" : "denied"}: ${messageOf(err)}`);
  }
}
