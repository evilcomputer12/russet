# russet-tool

Multi-call Xcode-tool shims for Swift Build and flutter_tools (ARCHITECTURE §3.1).

- Layer **F** (format/protocol library; permissive; may depend only on F crates) (ARCHITECTURE §5.1–§5.2). Licence: MIT.
- **Owner:** W16; `actool`/`ibtool`/`xcstringstool` modules W21, `ibtool` compile and link modes W21b. W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- Layer F binary: it may depend only on F crates (ARCHITECTURE §5.2).
