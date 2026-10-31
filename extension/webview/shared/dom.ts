// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// A small DOM builder for the vanilla webviews. Text always goes through
// textContent, never innerHTML, so runner and daemon strings stay inert.

type Attrs = Record<string, string | number | boolean | undefined | ((ev: Event) => void)>;
type Child = Node | string | false | null | undefined;

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === false) {
      continue;
    }
    if (typeof value === "function") {
      el.addEventListener(key.replace(/^on/, "").toLowerCase(), value);
    } else if (key === "class") {
      el.className = String(value);
    } else if (value === true) {
      el.setAttribute(key, "");
    } else {
      el.setAttribute(key, String(value));
    }
  }
  for (const child of children) {
    if (child === false || child === null || child === undefined) {
      continue;
    }
    el.append(typeof child === "string" ? document.createTextNode(child) : child);
  }
  return el;
}

/** Inject a stylesheet (CSP allows inline styles; scripts stay nonce-only). */
export function addStyles(...sheets: string[]): void {
  for (const css of sheets) {
    const style = document.createElement("style");
    style.textContent = css;
    document.head.append(style);
  }
}

/** Replace children while keeping focus on an element with the same data-focus-key. */
export function replaceKeepingFocus(parent: HTMLElement, ...children: Node[]): void {
  const active = document.activeElement as HTMLElement | null;
  const key = active && parent.contains(active) ? active.dataset.focusKey : undefined;
  parent.replaceChildren(...children);
  if (key) {
    const next = parent.querySelector<HTMLElement>(`[data-focus-key="${CSS.escape(key)}"]`);
    next?.focus();
  }
}

/** A polite live region for short announcements to screen readers. */
export function announcer(): (text: string) => void {
  const region = h("div", { class: "sr-only", role: "status", "aria-live": "polite" });
  document.body.append(region);
  return (text: string) => {
    region.textContent = "";
    // A tick so repeated identical messages are announced again.
    setTimeout(() => {
      region.textContent = text;
    }, 30);
  };
}
