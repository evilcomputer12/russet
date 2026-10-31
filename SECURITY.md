<!--
SPDX-License-Identifier: AGPL-3.0-only
SPDX-FileCopyrightText: 2026 Russet contributors
-->

# Security policy

## Reporting a vulnerability

Please report security problems **privately**, never in a public issue, discussion or pull request.

- **While the repository is private** (until the M1 gate): collaborators report directly to the maintainer,
  [@evilcomputer12](https://github.com/evilcomputer12), through a private channel, not in an issue.
- **Once the repository is public:** use GitHub's private vulnerability reporting (the repository's **Security**
  tab, **Report a vulnerability**). If that is unavailable, contact the maintainer through GitHub and ask for a
  private channel; do not include the details in the first message.

Include what you found, how to reproduce it, the affected version (`russet --version`) and platform, and the impact
you expect. Redact device identifiers, Apple IDs, team IDs, keys and tokens: a report never needs real ones.

## What happens next

- We acknowledge a report within 7 days and keep you updated at least every 14 days.
- We aim to release a fix within **90 days** of the report, sooner for actively exploited problems, and to publish
  an advisory (GitHub Security Advisories once public) together with a patch release.
- We credit reporters in the advisory unless they ask us not to.

## Supported versions

Russet is pre-1.0 (`0.x`). Only the latest release receives security fixes; there are no long-term branches yet.

## Scope

In scope, for example (ARCHITECTURE §12):

- the daemon's IPC (same-user only), the web endpoint on 127.0.0.1, tokens and `Origin`/`Host` checks;
- permission profiles, MCP tool tiers, out-of-band approvals and the audit log;
- the secret store (keyring master key, sealed files, passphrase fallback) and redaction in logs and bug reports;
- signing identities, App Store Connect keys and tokens, Apple ID sessions, anisette handling and the sandboxed ADI
  helper;
- debug forwards and the debugserver broker, pairing records, device data;
- release artifacts (checksums, minisign signatures, SBOM) and anything that would make Russet ship an Apple file.

Known limits stated to users (ARCHITECTURE §12.3) are not vulnerabilities by themselves; for example, an agent with
unrestricted shell access as the same user can run the `russet` CLI directly. Problems in third-party projects
(idevice, apple-codesign, pymobiledevice3, Code-OSS) belong to those projects; tell us too if Russet's use of them is
affected.
