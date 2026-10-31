// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, expect, it, vi } from "vitest";
import { AppleIdApi, classifySignInError, SignInController, type CallFn, type SignInMode, type SignInView } from "../../src/setup/appleId";

const CANARY_PASSWORD = "canary-Pa55word-7d1f";
const APPLE_ID = "synthetic.developer@example.invalid";
const ACCOUNT = { id: "acct-1", kind: "apple-id", label: APPLE_ID, teamId: "SYNTHTEAM1", isDefault: true };

function rpcError(code: number, russetCode?: string, message = "error"): Error {
  return Object.assign(new Error(message), { code, ...(russetCode ? { data: { russetCode } } : {}) });
}

type Handlers = Record<string, (params: unknown) => unknown>;

function harness(handlers: Handlers, opts: { mode?: SignInMode; consent?: boolean } = {}) {
  const calls: Array<{ method: string; params: unknown }> = [];
  const call: CallFn = async (method, params) => {
    calls.push({ method, params });
    const h = handlers[method];
    if (!h) {
      throw rpcError(-32601, undefined, "method not found");
    }
    return h(params);
  };
  let consent = opts.consent ?? true;
  const controller = new SignInController({
    api: new AppleIdApi(call),
    mode: () => opts.mode ?? "auto",
    consentGiven: () => consent,
    rememberConsent: () => {
      consent = true;
    },
  });
  const views: SignInView[] = [];
  controller.onDidChange((v) => views.push(v));
  return { controller, calls, views };
}

const twoFactorHandlers = (overrides: Handlers = {}): Handlers => ({
  "account.anisette.status": () => ({ kind: "local", librariesPresent: true, provisioned: true }),
  "account.appleId.begin": () => ({
    sessionId: "s-1",
    step: "two-factor",
    twoFactor: { via: "trustedDevice", numbers: [{ id: 1, masked: "+• (•••) •••-••12" }] },
  }),
  "account.appleId.requestCode": () => ({}),
  "account.appleId.submitCode": () => ACCOUNT,
  ...overrides,
});

describe("Apple ID sign-in", () => {
  it("signs in with a trusted-device code and shows the account and team", async () => {
    const { controller, calls } = harness(twoFactorHandlers());
    const signedIn = vi.fn();
    controller.onDidSignIn(signedIn);
    await controller.start();
    expect(controller.view).toEqual({ screen: "credentials", busy: false });
    await controller.submitCredentials(`  ${APPLE_ID} `, CANARY_PASSWORD);
    expect(controller.view).toMatchObject({ screen: "code", via: "trustedDevice", canText: true, busy: false });
    await controller.submitCode("12 34 56");
    expect(controller.view).toEqual({
      screen: "done",
      account: { label: APPLE_ID, kind: "apple-id", teamId: "SYNTHTEAM1" },
    });
    expect(signedIn).toHaveBeenCalledWith(ACCOUNT);
    expect(calls.map((c) => c.method)).toEqual([
      "account.anisette.status",
      "account.appleId.begin",
      "account.appleId.requestCode",
      "account.appleId.submitCode",
    ]);
    expect(calls[1].params).toEqual({ appleId: APPLE_ID, password: CANARY_PASSWORD, anisette: "local" });
    expect(calls[2].params).toEqual({ sessionId: "s-1", via: "trustedDevice" });
    expect(calls[3].params).toEqual({ sessionId: "s-1", code: "123456" });
  });

  it("never keeps the password: it reaches the RPC once and nothing else", async () => {
    const log = vi.spyOn(console, "log");
    const { controller, calls, views } = harness(twoFactorHandlers());
    await controller.start();
    await controller.submitCredentials(APPLE_ID, CANARY_PASSWORD);
    await controller.submitCode("123456");
    const withPassword = calls.filter((c) => JSON.stringify(c.params).includes(CANARY_PASSWORD));
    expect(withPassword.map((c) => c.method)).toEqual(["account.appleId.begin"]);
    expect(JSON.stringify(views)).not.toContain(CANARY_PASSWORD);
    expect(JSON.stringify(controller)).not.toContain(CANARY_PASSWORD);
    expect(Object.values(controller).map((v) => String(v)).join()).not.toContain(CANARY_PASSWORD);
    expect(log).not.toHaveBeenCalled();
    log.mockRestore();
  });

  it("texts the code to a trusted number instead", async () => {
    const { controller, calls } = harness(twoFactorHandlers());
    await controller.start();
    await controller.submitCredentials(APPLE_ID, CANARY_PASSWORD);
    controller.choosePhone();
    expect(controller.view).toEqual({ screen: "phones", numbers: [{ id: "1", masked: "+• (•••) •••-••12" }], busy: false });
    await controller.textMe("1");
    expect(controller.view).toMatchObject({ screen: "code", via: "sms", phone: "+• (•••) •••-••12", info: expect.stringMatching(/sent/) });
    expect(calls.at(-1)).toEqual({ method: "account.appleId.requestCode", params: { sessionId: "s-1", via: "sms", phoneId: 1 } });
    await controller.resend();
    expect(calls.at(-1)?.params).toEqual({ sessionId: "s-1", via: "sms", phoneId: 1 });
  });

  it("keeps the user on the code screen after a wrong code", async () => {
    const { controller } = harness(twoFactorHandlers({
      "account.appleId.submitCode": () => {
        throw rpcError(5251, "gsa.badCode");
      },
    }));
    await controller.start();
    await controller.submitCredentials(APPLE_ID, CANARY_PASSWORD);
    await controller.submitCode("12345");
    expect(controller.view).toMatchObject({ screen: "code", error: "Enter the 6-digit code." });
    await controller.submitCode("000000");
    expect(controller.view).toMatchObject({ screen: "code", busy: false, error: expect.stringMatching(/not correct/) });
  });

  it("reports a wrong password on the form", async () => {
    const { controller } = harness(twoFactorHandlers({
      "account.appleId.begin": () => {
        throw rpcError(5250, "gsa.badCredentials");
      },
    }));
    await controller.start();
    await controller.submitCredentials(APPLE_ID, "wrong");
    expect(controller.view).toEqual({ screen: "credentials", busy: false, error: "The Apple ID or password is not correct." });
  });

  it("finishes without two-factor when Apple does not ask for it", async () => {
    const { controller } = harness({
      "account.anisette.status": () => ({ kind: "local", librariesPresent: true, provisioned: true }),
      "account.appleId.begin": () => ({ sessionId: "s-2", step: "done" }),
      "account.list": () => ({ accounts: [ACCOUNT] }),
    });
    await controller.start();
    await controller.submitCredentials(APPLE_ID, CANARY_PASSWORD);
    expect(controller.view).toMatchObject({ screen: "done", account: { label: APPLE_ID } });
  });

  it("asks for consent and fetches the sign-in libraries when they are missing", async () => {
    const fetch = vi.fn().mockResolvedValue({ apkVersion: "x", bytesDownloaded: 2_000_000 });
    const { controller } = harness(
      {
        "account.anisette.status": () => ({ kind: "local", librariesPresent: false, provisioned: false }),
        "account.anisette.fetchLibraries": fetch,
      },
      { consent: false },
    );
    await controller.start();
    expect(controller.view).toEqual({ screen: "consent", librariesPresent: false, busy: false });
    await controller.acceptConsent();
    expect(fetch).toHaveBeenCalledWith({ consent: true });
    expect(controller.view).toEqual({ screen: "credentials", busy: false });
  });

  it("explains when the engine does not implement sign-in yet, unless forced on", async () => {
    const auto = harness({});
    await auto.controller.start();
    expect(auto.controller.view).toMatchObject({ screen: "unavailable", message: expect.stringMatching(/not available/) });

    const engineStub = harness({
      "account.anisette.status": () => {
        throw rpcError(1005, "core.notImplemented");
      },
    }, { mode: "on" });
    await engineStub.controller.start();
    expect(engineStub.controller.view).toEqual({ screen: "credentials", busy: false });
    await engineStub.controller.submitCredentials(APPLE_ID, CANARY_PASSWORD);
    expect(engineStub.controller.view).toMatchObject({ screen: "unavailable" });

    const off = harness({}, { mode: "off" });
    await off.controller.start();
    expect(off.controller.view).toMatchObject({ screen: "unavailable", message: expect.stringMatching(/turned off/) });
  });

  it("classifies daemon errors by code, never by message text", () => {
    expect(classifySignInError(rpcError(5252, "gsa.locked")).kind).toBe("locked");
    expect(classifySignInError(rpcError(1203, "perm.denied")).kind).toBe("notAllowed");
    expect(classifySignInError(rpcError(5107)).kind).toBe("serviceUnavailable");
    expect(classifySignInError(new Error("Russet daemon is not connected.")).kind).toBe("offline");
    expect(classifySignInError(rpcError(1003, "core.io", "disk")).kind).toBe("other");
  });
});
