# russet-agent-config

Writes MCP registrations for the agent clients (plan, apply, detect).

- Layer **L2** (may depend on L0–L1) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W13 (MCP-04). W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- The JSONC editing crate (ARCHITECTURE §5.1 "jsonc edits") is W13's choice.
- Layer L2 may not depend on russet-doctor (ARCHITECTURE §5.2); its doctor checks register from russetd.
