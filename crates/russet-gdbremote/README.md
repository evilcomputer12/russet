# russet-gdbremote

gdb-remote codec, stop-reply parser, async GdbClient and interposer hooks.

- Layer **F** (format/protocol library; permissive; may depend only on F crates) (ARCHITECTURE §5.1–§5.2). Licence: MIT OR Apache-2.0.
- **Owner:** W7 (RD-01).
- `codec`: `$payload#xx` framing with per-side ack checking, the raw interrupt, `}` escapes and `*` run-length decoding.
- `stop`: `T`/`S`/`W`/`X`/`O` replies with expedited registers, Mach exceptions and `jstopinfo`.
- `GdbClient`: the debugserver handshake, requests, one-write pipelines, `vAttach`/`vRun`, resume and stops,
  memory, breakpoints, registers, image and region queries, detach, kill.
- `interpose`: a frame relay between a debugger and a stub with separate ack state per side and hooks.
- `arm64`: register numbers and a `ret`/`bti`/`pac`/`nop` decoder for the JIT servicer.
