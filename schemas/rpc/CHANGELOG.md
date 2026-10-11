# russet/1 protocol changelog

Every change of the JSON-RPC protocol (ARCHITECTURE §7.7). Minor versions only add; a major version removes,
renames or changes meaning. The schema itself is generated (`cargo xtask schema`); this file is written by hand.

## 1.0 (in progress, freeze F3 at the end of Wave 1)

- W2: the wire DTOs of `initialize`, `daemon.*`, `subscription.*`, `lease.*`, `job.*`, `approval.*`, `audit.*`
  and `ui.*`, the shared references `ClientRef` and `ConnectionRef`, and the LSP built-ins `$/progress`
  (`ProgressParams`, `WorkDoneProgress` with `jobId`) and `$/cancelRequest`.
- W2: long-running methods accept `progressToken` and `background`; with `background: true` they return
  `BackgroundJob` (`{jobId}`).
- W2: `JobEvent` is `{seq, ts}` plus the `JobEventKind` of ARCHITECTURE §9.2 (`artifactKind`, never `kind`, for
  artifacts [D 96]).
- W2: a daemon that is shutting down keeps its endpoint until its jobs have had their grace and answers
  `initialize` with `ipc.daemonUnavailable` (1102, retryable) and `DaemonUnavailableDetails`
  (`{shuttingDown: true, graceMs}`); clients wait for it to go, then start a new daemon (core §6.3).
- W1: every method of ARCHITECTURE §8 and notification of §9 registered, with placeholder DTOs.
