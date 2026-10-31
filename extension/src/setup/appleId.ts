// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Native Apple ID sign-in (SIGN §13.1, ide.md §5.8) over the daemon's
// `account.appleId.*` and `account.anisette.*` methods (ARCHITECTURE §8,
// W22). This branch's generated protocol still types them `Unspecified`, so
// the shapes below are local copies of W22's DTOs (russet-protocol account.rs
// on ws/w22-apple-signin); swap them for the generated types when W22 merges.
// The ARCHITECTURE draft shapes (`via` as one string, `numbers[{id, masked}]`)
// are still read.
//
// Credentials: the password is passed straight from the page to
// `account.appleId.begin` and never stored, logged or kept on this object.
//
// Identity (anisette): this PC's own local ADI by default, or the remote
// server in `russet.account.anisetteServer` (user settings only), sent as
// `anisette: {remote: url}`; the remote path skips the local libraries.

import type { Account } from "../generated/protocol";

/** The page's names; on the wire W22 writes `trusted-device` (kebab-case). */
export type TwoFactorVia = "trustedDevice" | "sms";
type WireVia = "trusted-device" | "sms";

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
  /** W22 and ARCHITECTURE write `two-factor`; SIGN §12 drafted `twoFactor`. */
  step: "done" | "two-factor" | "twoFactor";
  twoFactor?: {
    /** W22: every way a code can be requested; the draft had one string. */
    via?: WireVia | "trustedDevice" | Array<WireVia | "trustedDevice">;
    /** W22: where the engine already sent the current code. */
    codeSentTo?: WireVia | "trustedDevice";
    /** W22: Apple's obfuscated trusted numbers. */
    phoneNumbers?: Array<{ id: number | string; number: string }>;
    /** The ARCHITECTURE draft's name for the same list. */
    numbers?: Array<{ id: string | number; masked: string }>;
    codeLength?: number;
  };
  /** W22: with `step: "done"`, the account that was added. */
  account?: Account;
}

export interface AnisetteStatus {
  kind: string;
  librariesPresent: boolean;
  apkVersion?: string | null;
  provisioned: boolean;
  /** W22: false when local ADI cannot run on this host. */
  localSupported?: boolean;
}

function fromWire(via: unknown): TwoFactorVia | undefined {
  return via === "trusted-device" || via === "trustedDevice" ? "trustedDevice" : via === "sms" ? "sms" : undefined;
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
      via: via === "trustedDevice" ? "trusted-device" : "sms",
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
  | "rateLimited"
  | "expired"
  | "unsupported"
  | "offline"
  | "other";

export interface SignInError {
  kind: SignInErrorKind;
  message: string;
  code?: string;
}

/**
 * Map a daemon error (ARCHITECTURE §13) to what the page tells the user.
 * `identityServer` is the remote anisette host in use, if any.
 */
export function classifySignInError(err: unknown, ctx: { identityServer?: string } = {}): SignInError {
  const e = err as {
    code?: unknown;
    message?: unknown;
    data?: { russetCode?: unknown; hint?: unknown; details?: { ec?: unknown; httpStatus?: unknown } };
  };
  const num = typeof e?.code === "number" ? e.code : undefined;
  const code = typeof e?.data?.russetCode === "string" ? e.data.russetCode : undefined;
  const hint = typeof e?.data?.hint === "string" ? e.data.hint : undefined;
  const raw = typeof e?.message === "string" ? e.message : String(err);
  const ec = e?.data?.details?.ec;
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
  if (num === 5253 || code === "gsa.identityRejected" || ec === -80045 || ec === -80046 || /\(-8004[56]\)/.test(raw)) {
    // GrandSlam refuses a client identity that does not match the anisette in use (-80045 at init, -80046 at
    // two-factor). A remote identity server presents its own matching identity.
    return withCode(
      "identityRejected",
      ctx.identityServer
        ? `Apple did not accept the sign-in identity from ${ctx.identityServer}. Try again later, or choose another identity server.`
        : "Apple did not accept this computer's own sign-in identity. You can sign in through an identity server instead (setting Russet › Account: Anisette Server).",
    );
  }
  if (e?.data?.details?.httpStatus === 503) {
    return withCode("serviceUnavailable", "Apple's sign-in service is busy (HTTP 503). Try again in a few minutes.");
  }
  if (num === 5254 || code === "gsa.codeRateLimited") {
    return withCode("rateLimited", "Apple is not sending more codes right now. Wait a while, then try again.");
  }
  if (num === 5258 || code === "gsa.loginExpired") {
    return withCode("expired", "The sign-in timed out. Enter your Apple ID and password again.");
  }
  if (num === 5276 || code === "anisette.unsupported") {
    return withCode("unsupported", "Apple ID sign-in cannot identify this computer to Apple yet (local anisette is not supported here).");
  }
  if (num === 5270 || num === 5273 || code === "anisette.libsMissing" || code === "anisette.consentRequired") {
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

/** `russet.account.anisetteServer`: empty for this PC's own (local) identity, else an https URL. */
export type IdentityServer =
  | { kind: "local" }
  | { kind: "remote"; url: string; host: string }
  | { kind: "invalid"; message: string };

export function parseIdentityServer(value: unknown): IdentityServer {
  const v = typeof value === "string" ? value.trim() : "";
  if (!v) {
    return { kind: "local" };
  }
  const invalid: IdentityServer = {
    kind: "invalid",
    message:
      "The identity server setting (Russet › Account: Anisette Server) must be an https:// URL such as https://server.example, or empty.",
  };
  let url: URL;
  try {
    url = new URL(v);
  } catch {
    return invalid;
  }
  if (url.protocol !== "https:" || !url.hostname || url.username || url.password) {
    return invalid;
  }
  return { kind: "remote", url: v, host: url.host };
}

export type SignInView =
  | { screen: "checking" }
  | { screen: "unavailable"; message: string; code?: string; suggestServer?: boolean }
  | { screen: "consent"; librariesPresent: boolean; busy: boolean; error?: string }
  | {
      screen: "credentials";
      busy: boolean;
      error?: string;
      /** The remote identity server's host, when one is configured. */
      identityServer?: string;
      /** Offer the identity-server setting (local identity rejected or unsupported). */
      suggestServer?: boolean;
    }
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
  /** The raw `russet.account.anisetteServer` value from user (machine) settings only. */
  identityServer: () => unknown;
}

const CODE = /^\d{6}$/;

export class SignInController {
  private current: SignInView = { screen: "checking" };
  private sessionId?: string;
  private numbers: TrustedNumber[] = [];
  private librariesPresent = true;
  private server: IdentityServer = { kind: "local" };
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

  private remoteHost(): string | undefined {
    return this.server.kind === "remote" ? this.server.host : undefined;
  }

  /** The Apple ID form, always naming the identity server when one is in use. */
  private credentials(extra: { busy?: boolean; error?: string; suggestServer?: boolean } = {}): SignInView {
    const host = this.remoteHost();
    return {
      screen: "credentials",
      busy: extra.busy ?? false,
      ...(extra.error ? { error: extra.error } : {}),
      ...(host ? { identityServer: host } : {}),
      ...(extra.suggestServer && !host ? { suggestServer: true } : {}),
    };
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
    this.server = parseIdentityServer(this.deps.identityServer());
    if (this.server.kind === "invalid") {
      this.set({ screen: "unavailable", message: this.server.message, code: "russet.account.anisetteServer" });
      return;
    }
    if (this.server.kind === "remote") {
      // The identity comes from the remote server: no local ADI libraries, provisioning or their consent.
      this.set(this.credentials());
      return;
    }
    this.set({ screen: "checking" });
    try {
      const status = await this.deps.api.anisetteStatus();
      this.librariesPresent = !!status.librariesPresent;
      if (status.localSupported === false && mode !== "on") {
        const e = classifySignInError({ code: 5276, data: { russetCode: "anisette.unsupported" } });
        this.set({ screen: "unavailable", message: e.message, code: "anisette.unsupported", suggestServer: true });
        return;
      }
    } catch (err) {
      const e = classifySignInError(err);
      if (e.kind === "notImplemented" && mode === "on") {
        // Forced on: the engine may sign in before its anisette status exists.
        this.librariesPresent = true;
      } else {
        this.set({
          screen: "unavailable",
          message: e.message,
          ...(e.code ? { code: e.code } : {}),
          ...(e.kind === "unsupported" ? { suggestServer: true } : {}),
        });
        return;
      }
    }
    if (!this.librariesPresent || !this.deps.consentGiven()) {
      this.set({ screen: "consent", librariesPresent: this.librariesPresent, busy: false });
    } else {
      this.set(this.credentials());
    }
  }

  /** The user agreed; fetch the anisette libraries if they are missing. */
  async acceptConsent(): Promise<void> {
    if (this.current.screen !== "consent" || this.busy()) {
      return;
    }
    await this.deps.rememberConsent();
    if (this.librariesPresent) {
      this.set(this.credentials());
      return;
    }
    this.set({ screen: "consent", librariesPresent: false, busy: true });
    try {
      await this.deps.api.fetchLibraries();
      this.librariesPresent = true;
      this.set(this.credentials());
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
      this.set(this.credentials({ error: "Enter your Apple ID and password." }));
      return;
    }
    // Read the setting again: it may have changed since the form opened.
    this.server = parseIdentityServer(this.deps.identityServer());
    if (this.server.kind === "invalid") {
      this.set(this.credentials({ error: this.server.message }));
      return;
    }
    const anisette: AppleIdBeginParams["anisette"] = this.server.kind === "remote" ? { remote: this.server.url } : "local";
    this.set(this.credentials({ busy: true }));
    let result: AppleIdBeginResult;
    try {
      result = await this.deps.api.begin({ appleId: id, password, anisette });
    } catch (err) {
      this.failCredentials(err);
      return;
    }
    if (!result || typeof result.sessionId !== "string") {
      this.set(this.credentials({ error: "The Russet engine returned an unexpected answer." }));
      return;
    }
    this.sessionId = result.sessionId;
    if (result.step === "done") {
      await this.finish(result.account);
      return;
    }
    const tf = result.twoFactor ?? {};
    const phones: Array<{ id: unknown; masked: unknown }> = [
      ...(tf.phoneNumbers ?? []).map((n) => ({ id: n?.id, masked: n?.number })),
      ...(tf.numbers ?? []).map((n) => ({ id: n?.id, masked: n?.masked })),
    ];
    this.numbers = phones
      .filter((n) => (typeof n.id === "string" || typeof n.id === "number") && typeof n.masked === "string")
      .map((n) => ({ id: String(n.id), masked: n.masked as string }));
    const offered = (Array.isArray(tf.via) ? tf.via : tf.via ? [tf.via] : []).map(fromWire);
    const sentTo = fromWire(tf.codeSentTo);
    const canText = this.numbers.length > 0;
    if (sentTo === "trustedDevice") {
      // The engine already asked Apple to push a code; requesting again would send a second one.
      this.set({ screen: "code", via: "trustedDevice", canText, busy: false });
    } else if (sentTo === "sms") {
      this.set({ screen: "code", via: "sms", ...(this.numbers[0] ? { phone: this.numbers[0].masked } : {}), canText, busy: false });
    } else if (offered.length === 0 || offered.includes("trustedDevice")) {
      await this.useTrustedDevice();
    } else if (canText) {
      await this.textMe(this.numbers[0].id);
    } else {
      this.set(this.credentials({ error: "Apple asked for a two-factor code but offered no way to send one." }));
    }
  }

  /** Errors on the code screens: an expired sign-in goes back to the form. */
  private failCode(err: unknown, fallback: SignInView): void {
    const e = classifySignInError(err, { identityServer: this.remoteHost() });
    if (e.kind === "expired" || e.kind === "identityRejected") {
      // -80046 rejects the identity at two-factor: the session is over either way.
      this.sessionId = undefined;
      this.set(this.credentials({ error: e.message, suggestServer: e.kind === "identityRejected" }));
      return;
    }
    this.set({ ...fallback, error: e.message } as SignInView);
  }

  private failCredentials(err: unknown): void {
    const e = classifySignInError(err, { identityServer: this.remoteHost() });
    if (e.kind === "notImplemented") {
      this.set({ screen: "unavailable", message: e.message, ...(e.code ? { code: e.code } : {}) });
    } else {
      this.set(this.credentials({ error: e.message, suggestServer: e.kind === "identityRejected" || e.kind === "unsupported" }));
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
      this.failCode(err, { screen: "code", via: "trustedDevice", canText, busy: false });
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
      this.failCode(err, { screen: "phones", numbers: this.numbers, busy: false });
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
      this.failCode(err, { ...view, busy: false, info: undefined });
    }
  }

  /** Start over from the Apple ID form. */
  back(): void {
    this.sessionId = undefined;
    this.numbers = [];
    this.set(this.credentials());
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
