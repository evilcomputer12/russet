// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The native Apple ID sign-in page: consent, Apple ID and password, two-factor
// code (trusted device or text message), then the signed-in account. The host's
// SignInController owns the flow. The password field is cleared as soon as it is
// sent and never written to webview state.

import { vscodeApi } from "../shared/vscode";
import { addStyles, h } from "../shared/dom";
import baseCss from "../shared/base.css?inline";
import pageCss from "./signin.css?inline";

type View =
  | { screen: "checking" }
  | { screen: "unavailable"; message: string; code?: string; suggestServer?: boolean }
  | { screen: "consent"; librariesPresent: boolean; busy: boolean; error?: string }
  | { screen: "credentials"; busy: boolean; error?: string; identityServer?: string; suggestServer?: boolean }
  | { screen: "code"; via: "trustedDevice" | "sms"; phone?: string; canText: boolean; busy: boolean; error?: string; info?: string }
  | { screen: "phones"; numbers: Array<{ id: string; masked: string }>; busy: boolean; error?: string }
  | { screen: "done"; account: { label: string; kind: string; teamId?: string } };

const vscode = vscodeApi();
addStyles(baseCss, pageCss);
const root = document.getElementById("root") as HTMLElement;
const post = (msg: unknown): void => vscode.postMessage(msg);

// Kept in memory only so a busy re-render does not wipe what the user typed.
let appleIdDraft = "";
let lastScreen = "";

function render(view: View): void {
  const screenChanged = view.screen !== lastScreen;
  lastScreen = view.screen;
  const main = h("main", { class: "signin", "aria-labelledby": "heading" });
  main.append(h("h1", { id: "heading", tabindex: -1 }, headingFor(view)));
  switch (view.screen) {
    case "checking":
      main.append(h("p", { class: "muted", role: "status" }, h("span", { class: "spin", "aria-hidden": "true" }, "◌"), " Checking the Russet engine…"));
      break;
    case "unavailable":
      main.append(
        h("div", { class: "alert info", role: "status" }, view.message, view.code ? h("span", { class: "muted" }, ` (${view.code})`) : ""),
        h("p", {}, "You can add an App Store Connect API key instead; it works with paid developer accounts."),
        h("div", { class: "row buttons" },
          view.suggestServer
            ? h("button", { onclick: () => post({ kind: "openIdentitySetting" }) }, "Use an identity server…")
            : "",
          h("button", { class: view.suggestServer ? "secondary" : "", onclick: () => post({ kind: "addKey" }) }, "Add App Store Connect API key…"),
          h("button", { class: "secondary", onclick: () => post({ kind: "restart" }) }, "Try again")),
      );
      break;
    case "consent":
      main.append(renderConsent(view));
      break;
    case "credentials":
      main.append(renderCredentials(view));
      break;
    case "code":
      main.append(renderCode(view));
      break;
    case "phones":
      main.append(renderPhones(view));
      break;
    case "done":
      main.append(renderDone(view));
      break;
  }
  root.replaceChildren(main);
  if (screenChanged) {
    const target =
      root.querySelector<HTMLElement>("[data-autofocus]") ?? root.querySelector<HTMLElement>("#heading");
    target?.focus();
  } else if ("error" in view && view.error) {
    root.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }
}

function headingFor(view: View): string {
  switch (view.screen) {
    case "code":
    case "phones":
      return "Two-factor authentication";
    case "done":
      return "You're signed in";
    default:
      return "Sign in with your Apple ID";
  }
}

function errorBox(text?: string): HTMLElement | "" {
  return text ? h("div", { class: "alert", role: "alert", id: "error" }, text) : "";
}

function renderConsent(view: Extract<View, { screen: "consent" }>): HTMLElement {
  const agree = h("input", { type: "checkbox", id: "agree", "data-autofocus": true, disabled: view.busy });
  const cont = h("button", { type: "submit", disabled: true }, view.busy ? "Downloading…" : "Continue");
  agree.addEventListener("change", () => {
    cont.disabled = !agree.checked || view.busy;
  });
  const form = h(
    "form",
    {
      class: "card",
      onsubmit: (ev: Event) => {
        ev.preventDefault();
        if (agree.checked) {
          post({ kind: "consent" });
        }
      },
    },
    h("ul", { class: "points" },
      h("li", {}, "Russet signs in to Apple directly from this PC, as Xcode does on a Mac. Your password is sent once to the Russet engine on this PC; the IDE never stores it."),
      h("li", {}, "Free Apple IDs can run apps on your own devices for 7 days at a time. A paid Apple Developer Program membership signs for a year and can publish."),
      h("li", {}, "Apple may ask you to confirm the sign-in on your iPhone or other trusted device."),
      view.librariesPresent
        ? ""
        : h("li", {}, "Russet first downloads Apple's sign-in support libraries (about 2 MB) to identify this PC to Apple.")),
    errorBox(view.error),
    h("div", { class: "check" }, agree, h("label", { for: "agree" }, "I understand and want to sign in with my Apple ID")),
    h("div", { class: "row buttons" }, cont,
      h("button", { type: "button", class: "secondary", onclick: () => post({ kind: "addKey" }) }, "Use an API key instead")),
  );
  if (view.busy) {
    form.setAttribute("aria-busy", "true");
  }
  return form;
}

function renderCredentials(view: Extract<View, { screen: "credentials" }>): HTMLElement {
  const describedBy = view.error ? "error" : "privacy";
  const id = h("input", {
    type: "text",
    id: "apple-id",
    name: "username",
    inputmode: "email",
    autocomplete: "username",
    spellcheck: "false",
    autocapitalize: "off",
    required: true,
    disabled: view.busy,
    "aria-describedby": describedBy,
    "aria-invalid": view.error ? "true" : undefined,
    "data-autofocus": !view.error || !appleIdDraft ? true : undefined,
  });
  id.value = appleIdDraft;
  id.addEventListener("input", () => {
    appleIdDraft = id.value;
  });
  const pw = h("input", {
    type: "password",
    id: "password",
    name: "password",
    autocomplete: "current-password",
    required: true,
    disabled: view.busy,
    "aria-describedby": describedBy,
    "aria-invalid": view.error ? "true" : undefined,
    "data-autofocus": view.error && appleIdDraft ? true : undefined,
  });
  return h(
    "form",
    {
      class: "card",
      "aria-busy": view.busy ? "true" : undefined,
      onsubmit: (ev: Event) => {
        ev.preventDefault();
        const password = pw.value;
        // Clear first, so the field never holds the password after it is sent.
        pw.value = "";
        post({ kind: "credentials", appleId: id.value, password });
      },
    },
    errorBox(view.error),
    h("label", { for: "apple-id" }, "Apple ID (email or phone number)"),
    id,
    h("label", { for: "password" }, "Password"),
    pw,
    h("p", { class: "muted small", id: "privacy" }, "Sent once to the Russet engine on this PC. Russet never stores your password in the IDE or in settings."),
    view.identityServer
      ? h("p", { class: "muted small identity", role: "note" },
          "Sign-in identity from ", h("strong", {}, view.identityServer),
          ". Its operator sees this computer's Apple device-identity requests, not your password or codes. ",
          h("button", { type: "button", class: "link", onclick: () => post({ kind: "openIdentitySetting" }) }, "Change"))
      : "",
    view.suggestServer
      ? h("p", { class: "small" },
          h("button", { type: "button", class: "link", onclick: () => post({ kind: "openIdentitySetting" }) }, "Use an identity server…"))
      : "",
    h("div", { class: "row buttons" },
      h("button", { type: "submit", disabled: view.busy }, view.busy ? "Signing in…" : "Sign In"),
      h("button", { type: "button", class: "link", onclick: () => post({ kind: "openExternal", target: "iforgot" }) }, "Forgot Apple ID or password?")),
  );
}

function renderCode(view: Extract<View, { screen: "code" }>): HTMLElement {
  const help = view.via === "sms"
    ? `Enter the 6-digit code sent to ${view.phone ?? "your phone"}.`
    : "Enter the 6-digit code shown on your iPhone, iPad or Mac.";
  const input = h("input", {
    type: "text",
    id: "code",
    inputmode: "numeric",
    autocomplete: "one-time-code",
    maxlength: 6,
    pattern: "[0-9]{6}",
    class: "code-input",
    required: true,
    disabled: view.busy,
    "aria-describedby": view.error ? "error code-help" : "code-help",
    "aria-invalid": view.error ? "true" : undefined,
    "data-autofocus": true,
  });
  const submit = (): void => {
    const code = input.value;
    input.value = "";
    post({ kind: "code", code });
  };
  input.addEventListener("input", () => {
    input.value = input.value.replace(/\D/g, "").slice(0, 6);
    if (input.value.length === 6) {
      submit();
    }
  });
  return h(
    "form",
    {
      class: "card",
      "aria-busy": view.busy ? "true" : undefined,
      onsubmit: (ev: Event) => {
        ev.preventDefault();
        submit();
      },
    },
    view.info ? h("div", { class: "alert info", role: "status" }, view.info) : "",
    errorBox(view.error),
    h("label", { for: "code" }, "Verification code"),
    input,
    h("p", { class: "muted small", id: "code-help" }, help),
    h("div", { class: "row buttons" },
      h("button", { type: "submit", disabled: view.busy }, view.busy ? "Verifying…" : "Verify"),
      h("button", { type: "button", class: "secondary", disabled: view.busy, onclick: () => post({ kind: "resend" }) }, "Send a new code")),
    h("div", { class: "row alt" },
      view.via === "trustedDevice" && view.canText
        ? h("button", { type: "button", class: "link", disabled: view.busy, onclick: () => post({ kind: "choosePhone" }) }, "Didn't get a code? Text me instead")
        : "",
      view.via === "sms"
        ? h("button", { type: "button", class: "link", disabled: view.busy, onclick: () => post({ kind: "trustedDevice" }) }, "Use my trusted device instead")
        : "",
      h("button", { type: "button", class: "link", disabled: view.busy, onclick: () => post({ kind: "back" }) }, "Start over")),
  );
}

function renderPhones(view: Extract<View, { screen: "phones" }>): HTMLElement {
  const fs = h("fieldset", {}, h("legend", {}, "Send the code to"));
  view.numbers.forEach((n, i) => {
    const id = `phone-${i}`;
    fs.append(
      h("div", { class: "radio" },
        h("input", { type: "radio", name: "phone", id, value: n.id, checked: i === 0, "data-autofocus": i === 0 ? true : undefined }),
        h("label", { for: id }, n.masked)),
    );
  });
  return h(
    "form",
    {
      class: "card",
      "aria-busy": view.busy ? "true" : undefined,
      onsubmit: (ev: Event) => {
        ev.preventDefault();
        const picked = (ev.currentTarget as HTMLFormElement).querySelector<HTMLInputElement>("input[name=phone]:checked");
        if (picked) {
          post({ kind: "phone", id: picked.value });
        }
      },
    },
    errorBox(view.error),
    fs,
    h("div", { class: "row buttons" },
      h("button", { type: "submit", disabled: view.busy }, view.busy ? "Sending…" : "Text Me the Code"),
      h("button", { type: "button", class: "secondary", onclick: () => post({ kind: "trustedDevice" }) }, "Back")),
  );
}

function renderDone(view: Extract<View, { screen: "done" }>): HTMLElement {
  const kind = view.account.kind === "apple-id" ? "Apple ID" : "App Store Connect API key";
  return h(
    "div",
    { class: "card done", role: "status" },
    h("p", { class: "ok" }, h("span", { "aria-hidden": "true" }, "✓ "), "Russet can now sign and install your apps."),
    h("dl", {},
      h("dt", {}, "Account"), h("dd", {}, view.account.label),
      h("dt", {}, "Type"), h("dd", {}, kind),
      view.account.teamId ? h("dt", {}, "Team") : "", view.account.teamId ? h("dd", {}, view.account.teamId) : ""),
    h("div", { class: "row buttons" },
      h("button", { "data-autofocus": true, onclick: () => post({ kind: "close" }) }, "Done"),
      h("button", { class: "secondary", onclick: () => post({ kind: "openSigning" }) }, "Open Signing")),
  );
}

window.addEventListener("message", (event: MessageEvent) => {
  const msg = event.data as { kind?: string; view?: View };
  if (msg.kind === "view" && msg.view) {
    render(msg.view);
  }
});

render({ screen: "checking" });
post({ kind: "ready" });
