# russet-lab

Device-in-the-loop harness and suites (L7).

- Layer **dev** (tests and tooling; never a normal dependency of a shipped crate) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W9 (lane A only). W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- Lane A only: device suites run through `cargo xtask lab` under the live-device lock (ARCHITECTURE §3.3 item 7).

## Running (lane A)

```sh
export RUSSET_LANE=A RUSSET_HOME="$HOME/russet-lab/home-A"
cargo xtask lab smoke                    # E2E-01, 02, 03, 04, 06, 14 (tests/smoke.rs), serially
cargo xtask lab exit-script --runs 3     # demo/m1-russet.sh three times in a row (DC-05; reruns must skip the install)
cargo xtask lab dc28                     # tests/lab/dc28-ddi.sh (gate item 7; RUSSET_DC28_FORCE=owner-approved to replace a DDI)
```

`cargo xtask lab` builds `russet`, passes it as `RUSSET_BIN`, and exits 3 (skipped environment) outside lane A or
while a daemon of another `RUSSET_HOME` holds the live-device lock. The suites need the owner-local
`tests/fixtures/apps/RussetProbe/russet.local.toml` (PLAN §4.10). Results go to `docs/lab/results/`.
