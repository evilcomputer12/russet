// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The issuer ID asked for when adding an App Store Connect API key. A team
// key's issuer ID is a UUID (App Store Connect › Users and Access ›
// Integrations); an individual key has none. The daemon only rejects a wrong
// value after the dialog closes, so the input box checks it while the user
// types and trims what a paste carries along.

const ISSUER_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export const ISSUER_ID_PROMPT =
  "Team key: the Issuer ID shown above the keys in App Store Connect › Users and Access › Integrations. " +
  "Leave it empty for an individual key (Apple does not let those manage certificates or profiles).";

export const ISSUER_ID_PLACEHOLDER = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx";

/** Why `value` cannot be an issuer ID, or undefined when it can (empty: an individual key). */
export function issuerIdProblem(value: string): string | undefined {
  const v = value.trim();
  if (!v || ISSUER_ID.test(v)) {
    return undefined;
  }
  if (/^[A-Z0-9]{10}$/i.test(v)) {
    return `That looks like a 10-character key or team ID. The issuer ID is a UUID: ${ISSUER_ID_PLACEHOLDER}.`;
  }
  return `An issuer ID is a UUID: ${ISSUER_ID_PLACEHOLDER}.`;
}

/** The `issuerId` parameter of `account.addAscKey`: trimmed, or undefined for an individual key. */
export function issuerIdParam(value: string): string | undefined {
  return value.trim() || undefined;
}
