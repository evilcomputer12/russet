# Russet Windows host work

This is the separate local Windows host/supervisor experiment. Preserve the Rust
engine in `../russet`; do not copy engine source into this tree. No remote has
been created. Do not publish, push, change licensing, or trust certificates without
the owner's authorization.

Windows is primary. Keep both physical and embedded interactive virtual iPhones
in the first release acceptance gate: build, install, launch, source breakpoint,
variables, step and continue. Code-OSS/editor reuse is the initial recommendation;
do not replace it with a new IDE without evidence and presenting the comparison.

Use native Windows tools for the host and a separately named managed WSL2 distro
for the backend. Do not alter the user's Ubuntu, default distro, global WSL
configuration, firewall, Linux reference checkout, original VM disks, or device
ownership. No global WSL shutdown or unregister as cleanup. Keep secrets, device
identifiers, transcripts, SDKs, firmware and VM images out of source/packages.

Engine ownership and device lanes in `../russet/AGENTS.md` still apply. Device
checks require lane A and its existing lock; development uses backend `none` or
scripted fixtures, isolated RUSSET_HOME and memory secrets. Respect the 500 MB
download approval boundary; inspect installed prerequisites first. Every claimed
acceptance needs actual evidence. Transport fixtures alone do not prove debugging.
