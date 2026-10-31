#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Russet contributors
#
# Tier 0: launch the Russet extension in VS Code 1.141 with an isolated profile,
# without installing it into the user's main profile (ide.md PLAN W14, EXT-01/IDE-03).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
code_bin="${RUSSET_CODE_BIN:-code}"
profile_dir="${RUSSET_DEV_PROFILE:-$here/.dev-profile}"

echo "[dev-ext] building…"
( cd "$here" && npm run compile )

mkdir -p "$profile_dir/user-data" "$profile_dir/extensions"

echo "[dev-ext] launching $code_bin with an isolated profile at $profile_dir"
exec "$code_bin" \
  --user-data-dir "$profile_dir/user-data" \
  --extensions-dir "$profile_dir/extensions" \
  --extensionDevelopmentPath="$here" \
  "${@:-$here}"
