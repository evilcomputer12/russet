// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, it, expect, vi, beforeEach } from "vitest";
import { ResponseError } from "vscode-jsonrpc/node";

const showWarningMessage = vi.fn();
vi.mock("vscode", () => ({
  ProgressLocation: { Notification: 15 },
  window: {
    showWarningMessage: (...a: unknown[]) => showWarningMessage(...a),
    showErrorMessage: vi.fn(),
    withProgress: (_o: unknown, task: () => unknown) => task(),
  },
}));

const { describeChanges, makeAccountChanges, needsAccountChanges } = await import("../../src/run/accountChanges");
const { describeApproval } = await import("../../src/daemon/approvals");

const approvalRequired = (message: string, details?: object): ResponseError<unknown> =>
  new ResponseError(1204, message, { russetCode: "perm.approvalRequired", retryable: false, chain: [message], ...(details ? { details } : {}) });

const PLAN = {
  planId: "pln_1",
  digest: "d",
  accountId: "apple:1",
  teamId: "T…",
  distribution: "development",
  mutations: [
    { kind: "registerDevice", summary: "register the device iPhone (…a1b2)" },
    { kind: "createProfile", summary: "create a development profile for com.example.test" },
  ],
  profiles: [],
  warnings: [],
};

describe("Run's account changes", () => {
  beforeEach(() => showWarningMessage.mockReset());

  it("tells Run's provisioning stop from an approval request that has an id", () => {
    expect(needsAccountChanges(approvalRequired("provisioning needs 2 Apple-side change(s)"))).toBe(true);
    expect(needsAccountChanges(approvalRequired("applying plan pln_1 needs a human approval", { approvalId: "apr_1" }))).toBe(false);
    expect(needsAccountChanges(new Error("other"))).toBe(false);
  });

  it("lists the changes for the user", () => {
    expect(describeChanges(PLAN.mutations as never)).toEqual({
      message: "Running on this iPhone needs 2 changes in your Apple developer account",
      detail: "• register the device iPhone (…a1b2)\n• create a development profile for com.example.test\n\nRusset makes only these changes, then runs the app.",
    });
  });

  it("makes them once the user agrees: plan, apply, the daemon's approval granted, apply with it", async () => {
    const calls: Array<[string, unknown]> = [];
    let applied = 0;
    const call = vi.fn(async (method: string, params: unknown) => {
      calls.push([method, params]);
      if (method === "provision.plan") return PLAN;
      if (method === "provision.apply" && applied++ === 0) throw approvalRequired("needs a human approval", { approvalId: "apr_7" });
      return {};
    });
    showWarningMessage.mockResolvedValue("Make the Changes");
    await expect(makeAccountChanges({ call } as never, "C:\\Users\\someone\\Test", "UDID-1")).resolves.toBe("made");
    expect(calls).toEqual([
      ["provision.plan", { project: "C:\\Users\\someone\\Test", distribution: "development", devices: ["UDID-1"] }],
      ["provision.apply", { planId: "pln_1" }],
      ["approval.grant", { approvalId: "apr_7", scope: "once" }],
      ["provision.apply", { planId: "pln_1", approval: "apr_7" }],
    ]);
    expect(showWarningMessage.mock.calls[0][1]).toMatchObject({ modal: true });
  });

  it("changes nothing when the user says no", async () => {
    const call = vi.fn(async (method: string) => (method === "provision.plan" ? PLAN : {}));
    showWarningMessage.mockResolvedValue(undefined);
    await expect(makeAccountChanges({ call } as never, "/p", "UDID-1")).resolves.toBe("declined");
    expect(call.mock.calls.map((c) => c[0])).toEqual(["provision.plan"]);
  });

  it("asks an agent's request in the IDE's own words", () => {
    const { message, detail } = describeApproval(
      {
        approvalId: "apr_1",
        state: "pending",
        requestedBy: { kind: "mcp", name: "claude-code", connectionId: "c-3" },
        subject: { kind: "plan", planId: "pln_1", planDigest: "d" },
        tier: 2,
        summary: "apply provisioning plan pln_1 (1 account change(s))",
        detail: "register the device iPhone (…a1b2)",
        createdAt: "2026-11-06T10:00:00Z",
        expiresAt: "2026-11-06T10:10:00Z",
      } as never,
      new Date("2026-11-06T10:01:00Z"),
    );
    expect(message).toBe('The AI agent "claude-code" asks for your approval: apply provisioning plan pln_1 (1 account change(s))');
    expect(detail).toMatch(/^register the device iPhone/);
    expect(detail).toMatch(/expires in 9 min\.$/);
  });
});
