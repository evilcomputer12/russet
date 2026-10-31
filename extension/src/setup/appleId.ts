// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Native Apple ID sign-in (SIGN §13.1, ide.md §5.8) over the daemon's
// `account.appleId.*` and `account.anisette.*` methods (ARCHITECTURE §8,
// W22). The generated protocol still types them `Unspecified`, so the shapes
// below are local copies of the ARCHITECTURE table; swap them for the generated
// types when W22 lands.
//
// Credentials: the password is passed straight from the page to
// `account.appleId.begin` and never stored, logged or kept on this object.

import type { Account } from "../generated/protocol";

export type TwoFactorVia = "trustedDevice" | "sms";

export interface TrustedNumber {
  id: string;
  masked: string;
}

export interface AppleIdBeginParams {
  appleId: string;
  password: string;
  anisette: "local" | { remote: string };
}

export interface AppleIdBeginResult {
  sessionId: string;
  /** ARCHITECTURE writes `two-factor`, SIGN §12 `twoFactor`; both are accepted. */
  step: "done" | "two-factor" | "twoFactor";
  twoFactor?: { via?: TwoFactorVia; numbers?: Array<{ id: string | number; masked: string }> };
}

export interface AnisetteStatus {
  kind: string;
  librariesPresent: boolean;
  apkVersion?: string;
  provisioned: boolean;
}

export type CallFn = (method: string, params: unknown) => Promise<unknown>;

export class AppleIdApi {
  constructor(private readonly call: CallFn) {}

  anisetteStatus(): Promise<AnisetteStatus> {
    return this.call("account.anisette.status", {}) as Promise<AnisetteStatus>;
  }

  fetchLibraries(): Promise<{ apkVersion: string; bytesDownloaded: number }> {
    return this.call("account.anisette.fetchLibraries", { consent: true }) as Promise<{
      apkVersion: string;
      bytesDownloaded: number;
    }>;
  }

  begin(params: AppleIdBeginParams): Promise<AppleIdBeginResult> {
    return this.call("account.appleId.begin", params) as Promise<AppleIdBeginResult>;
  }

  requestCode(sessionId: string, via: TwoFactorVia, phoneId?: string): Promise<unknown> {
    return this.call("account.appleId.requestCode", {
      sessionId,
      via,
      ...(phoneId === undefined ? {} : { phoneId: /^\d+$/.test(phoneId) ? Number(phoneId) : phoneId }),
    });
  }

  submitCode(sessionId: string, code: string): Promise<Account> {
    return this.call("account.appleId.submitCode", { sessionId, code }) as Promise<Account>;
  }

  async accounts(): Promise<Account[]> {
    const res = (await this.call("account.list", {})) as { accounts?: Account[] };
    return res.accounts ?? [];
  }
}

export type SignInErrorKind =
  | "notImplemented"
  | "badCredentials"
  | "badCode"
  | "locked"
  | "identityRejected"
  | "notAllowed"
  | "librariesMissing"
  | "serviceUnavailable"
  | "offline"
  | "other";

export interface SignInError {
  kind: SignInErrorKind;
  message: string;
  code?: string;
}

/** Map a daemon error (ARCHITECTURE §13) to what the page tells the user. */
export function classifySignInError(err: unknown): SignInError {
  const e = err as { code?: unknown; message?: unknown; data?: { russetCode?: unknown; hint?: unknown } };
  const num = typeof e?.code === "number" ? e.code : undefined;
  const code = typeof e?.data?.russetCode === "string" ? e.data.russetCode : undefined;
  const hint = typeof e?.data?.hint === "string" ? e.data.hint : undefined;
  const raw = typeof e?.message === "string" ? e.message : String(err);
  const withCode = (kind: SignInErrorKind, message: string): SignInError => ({
    kind,
    message,
    ...(code ? { code } : num !== undefined ? { code: String(num) } : {}),
  });
  if (num === 1005 || num === -32601 || num === 3013 || code === "core.notImplemented" || code === "service.not_implemented") {
    return withCode("notImplemented", "Apple ID sign-in is not available in this version of the Russet engine yet.");
  }
  if (num === 5250 || code === "gsa.badCredentials") {
    return withCode("badCredentials", "The Apple ID or password is not correct.");
  }
  if (num === 5251 || code === "gsa.badCode") {
    return withCode("badCode", "That code is not correct or has expired. Check the code and try again.");
  }
  if (num === 5252 || code === "gsa.locked") {
    return withCode("locked", hint ?? "This Apple ID is locked. Unlock it at iforgot.apple.com, then try again.");
  }
  if (num === 5253 || code === "gsa.identityRejected") {
    return withCode("identityRejected", hint ?? "Apple did not accept this computer's sign-in identity. Try again later.");
  }
  if (num === 5270 || code === "anisette.libsMissing") {
    return withCode("librariesMissing", "The Apple sign-in libraries are missing. Download them to continue.");
  }
  if (num === 5107 || code === "account.appleServiceUnavailable") {
    return withCode("serviceUnavailable", "Apple's sign-in service is not responding. Try again in a few minutes.");
  }
  if ((num !== undefined && num >= 1200 && num < 1300) || code?.startsWith("perm.")) {
    return withCode(
      "notAllowed",
      "The Russet engine refused sign-in from the IDE connection. Update Russet, then try again.",
    );
  }
  if (/not connected/i.test(raw)) {
    return withCode("offline", "The Russet engine is not running. Finish the setup steps, then try again.");
  }
  return withCode("other", hint ? `${raw} ${hint}` : raw);
}

export interface SignedInSummary {
  label: string;
  kind: string;
  teamId?: string;
}

export type SignInView =
  | { screen: "checking" }
  | { screen: "unavailable"; message: string; code?: string }
  | { screen: "consent"; librariesPresent: boolean; busy: boolean; error?: string }
  | { screen: "credentials"; busy: boolean; error?: string }
  | {
      screen: "code";
      via: TwoFactorVia;
      phone?: string;
      canText: boolean;
      busy: boolean;
      error?: string;
      info?: string;
    }
  | { screen: "phones"; numbers: TrustedNumber[]; busy: boolean; error?: string }
  | { screen: "done"; account: SignedInSummary };

export type SignInMode = "auto" | "on" | "off";

export interface SignInControllerDeps {
  api: AppleIdApi;
  mode: () => SignInMode;
  consentGiven: () => boolean;
  rememberConsent: () => void | Thenable<void>;
}

const CODE = /^\d{6}$/;

export class SignInController {
  private current: SignInView = { screen: "checking" };
  private sessionId?: string;
  private numbers: TrustedNumber[] = [];
  private librariesPresent = true;
  private readonly listeners = new Set<(view: SignInView) => void>();
  private readonly signedIn = new Set<(account: Account) => void>();

  constructor(private readonly deps: SignInControllerDeps) {}

  get view(): SignInView {
    return this.current;
  }

  onDidChange(listener: (view: SignInView) => void): { dispose(): void } {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  onDidSignIn(listener: (account: Account) => void): { dispose(): void } {
    this.signedIn.add(listener);
    return { dispose: () => this.signedIn.delete(listener) };
  }

  private set(view: SignInView): void {
    this.current = view;
    for (const l of [...this.listeners]) {
      l(view);
    }
  }

  private busy(): boolean {
    return "busy" in this.current && this.current.busy;
  }

  /** Check the engine and the sign-in libraries, then show consent or the form. */
  async start(): Promise<void> {
    this.sessionId = undefined;
    this.numbers = [];
    const mode = this.deps.mode();
    if (mode === "off") {
      this.set({
        screen: "unavailable",
        message: "Apple ID sign-in is turned off (setting russet.account.appleIdSignIn).",
      });
      return;
    }
    this.set({ screen: "checking" });
    try {
      const status = await this.deps.api.anisetteStatus();
      this.librariesPresent = !!status.librariesPresent;
    } catch (err) {
      const e = classifySignInError(err);
      if (e.kind === "notImplemented" && mode === "on") {
        // Forced on: the engine may sign in before its anisette status exists.
        this.librariesPresent = true;
      } else {
        this.set({ screen: "unavailable", message: e.message, ...(e.code ? { code: e.code } : {}) });
        return;
      }
    }
    if (!this.librariesPresent || !this.deps.consentGiven()) {
      this.set({ screen: "consent", librariesPresent: this.librariesPresent, busy: false });
    } else {
      this.set({ screen: "credentials", busy: false });
    }
  }

  /** The user agreed; fetch the anisette libraries if they are missing. */
  async acceptConsent(): Promise<void> {
    if (this.current.screen !== "consent" || this.busy()) {
      return;
    }
    await this.deps.rememberConsent();
    if (this.librariesPresent) {
      this.set({ screen: "credentials", busy: false });
      return;
    }
    this.set({ screen: "consent", librariesPresent: false, busy: true });
    try {
      await this.deps.api.fetchLibraries();
      this.librariesPresent = true;
      this.set({ screen: "credentials", busy: false });
    } catch (err) {
      this.set({ screen: "consent", librariesPresent: false, busy: false, error: classifySignInError(err).message });
    }
  }

  /** Send the Apple ID and password once to `account.appleId.begin`. */
  async submitCredentials(appleId: string, password: string): Promise<void> {
    if (this.current.screen !== "credentials" || this.busy()) {
      return;
    }
    const id = appleId.trim();
    if (!id || !password) {
      this.set({ screen: "credentials", busy: false, error: "Enter your Apple ID and password." });
      return;
    }
    this.set({ screen: "credentials", busy: true });
    let result: AppleIdBeginResult;
    try {
      result = await this.deps.api.begin({ appleId: id, password, anisette: "local" });
    } catch (err) {
      this.failCredentials(err);
      return;
    }
    if (!result || typeof result.sessionId !== "string") {
      this.set({ screen: "credentials", busy: false, error: "The Russet engine returned an unexpected answer." });
      return;
    }
    this.sessionId = result.sessionId;
    if (result.step === "done") {
      await this.finish();
      return;
    }
    this.numbers = (result.twoFactor?.numbers ?? [])
      .filter((n) => n && (typeof n.id === "string" || typeof n.id === "number") && typeof n.masked === "string")
      .map((n) => ({ id: String(n.id), masked: n.masked }));
    if (result.twoFactor?.via === "sms" && this.numbers.length > 0) {
      await this.textMe(this.numbers[0].id);
      return;
    }
    await this.useTrustedDevice();
  }

  private failCredentials(err: unknown): void {
    const e = classifySignInError(err);
    if (e.kind === "notImplemented") {
      this.set({ screen: "unavailable", message: e.message, ...(e.code ? { code: e.code } : {}) });
    } else {
      this.set({ screen: "credentials", busy: false, error: e.message });
    }
  }

  /** Ask Apple to show a code on the user's trusted devices. */
  async useTrustedDevice(): Promise<void> {
    if (!this.sessionId) {
      return;
    }
    const canText = this.numbers.length > 0;
    this.set({ screen: "code", via: "trustedDevice", canText, busy: true });
    try {
      await this.deps.api.requestCode(this.sessionId, "trustedDevice");
      this.set({ screen: "code", via: "trustedDevice", canText, busy: false });
    } catch (err) {
      this.set({ screen: "code", via: "trustedDevice", canText, busy: false, error: classifySignInError(err).message });
    }
  }

  /** "Text me instead": show the trusted numbers. */
  choosePhone(): void {
    if (!this.sessionId || this.numbers.length === 0) {
      return;
    }
    this.set({ screen: "phones", numbers: this.numbers, busy: false });
  }

  async textMe(phoneId: string): Promise<void> {
    const phone = this.numbers.find((n) => n.id === phoneId);
    if (!this.sessionId || !phone) {
      return;
    }
    this.set({ screen: "code", via: "sms", phone: phone.masked, canText: true, busy: true });
    try {
      await this.deps.api.requestCode(this.sessionId, "sms", phone.id);
      this.set({
        screen: "code",
        via: "sms",
        phone: phone.masked,
        canText: true,
        busy: false,
        info: `A code was sent to ${phone.masked}.`,
      });
    } catch (err) {
      this.set({ screen: "phones", numbers: this.numbers, busy: false, error: classifySignInError(err).message });
    }
  }

  /** Re-send the code the same way. */
  async resend(): Promise<void> {
    if (this.current.screen !== "code" || this.busy()) {
      return;
    }
    if (this.current.via === "sms") {
      const phone = this.numbers.find((n) => n.masked === (this.current as { phone?: string }).phone);
      if (phone) {
        await this.textMe(phone.id);
      }
      return;
    }
    await this.useTrustedDevice();
  }

  async submitCode(code: string): Promise<void> {
    if (this.current.screen !== "code" || this.busy() || !this.sessionId) {
      return;
    }
    const view = this.current;
    const trimmed = code.replace(/\s+/g, "");
    if (!CODE.test(trimmed)) {
      this.set({ ...view, busy: false, info: undefined, error: "Enter the 6-digit code." });
      return;
    }
    this.set({ ...view, busy: true, error: undefined, info: undefined });
    try {
      const account = await this.deps.api.submitCode(this.sessionId, trimmed);
      await this.finish(account);
    } catch (err) {
      const e = classifySignInError(err);
      this.set({ ...view, busy: false, info: undefined, error: e.message });
    }
  }

  /** Start over from the Apple ID form. */
  back(): void {
    this.sessionId = undefined;
    this.numbers = [];
    this.set({ screen: "credentials", busy: false });
  }

  private async finish(account?: Account): Promise<void> {
    this.sessionId = undefined;
    let acct = account;
    if (!acct || typeof acct.label !== "string") {
      try {
        const all = await this.deps.api.accounts();
        acct = all.find((a) => a.kind === "apple-id" && a.isDefault) ?? all.find((a) => a.kind === "apple-id") ?? all[0];
      } catch {
        acct = undefined;
      }
    }
    const summary: SignedInSummary = acct
      ? { label: acct.label, kind: acct.kind, ...(acct.teamId ? { teamId: acct.teamId } : {}) }
      : { label: "Apple ID", kind: "apple-id" };
    this.set({ screen: "done", account: summary });
    if (acct) {
      for (const l of [...this.signedIn]) {
        l(acct);
      }
    }
  }
}
