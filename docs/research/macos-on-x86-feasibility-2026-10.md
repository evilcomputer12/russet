# macOS 27 "Golden Gate" on an x86_64 PC: feasibility and bring-up plan

| | |
|---|---|
| Project | `~/RE/goldengate` (personal interoperability research) |
| Host | AMD Ryzen 5 5600X (6C/12T, Zen 3), 31 GiB RAM, Radeon RX 7600 XT 16 GB (RADV), CachyOS, kernel 7.2, no sudo |
| Target | macOS 27.0 / 27.0.1 (builds 26A428 / 26A434, **Darwin 27.0.0**, xnu-13432.1.9) |
| Date | 2026-10-11 (host snapshot taken 2026-10-10, 23:40 to 23:47 CEST) |
| Status | Research synthesis. Nothing has been booted yet. |

**How to read this.** §1 gives the answer in plain words. §2 lists the facts it rests on. §3 and §4 compare approaches and lay out the plan. §5 to §8 are engineering detail, §9 is the risk register and §10 holds the questions only you can answer.

**Source key.** IDs such as `vm-13` point to the verified research records of this workflow run. Each ID also gives the primary evidence (file under `~/RE/goldengate/…` or a URL).
- **vm**: VirtualMac2,1 IPSW recon, in `recon/27.0.1/`, `recon/vmapple-diff/`.
- **proj**: prior art, in `recon/prior-art/`.
- **x86**: x86_64 code left in macOS 27, in `recon/x86code/`.
- **xlat**: binary translation and performance, in `recon/xlat/`.
- **boot**: boot chain and disk, in `recon/27.0.1/boot/`, `recon/27.0/tart/`.
- **gfx**: graphics and input, in `recon/gfx/`.

Other markers:
- **+fc**: a fact the fact-checker added and verified.
- **[S]**: verified in this session while writing the plan (Appendix A lists commands and artifacts).

Only confirmed or corrected facts are used as facts; with a corrected claim, the corrected wording is used.

---

## 1. Executive summary

**Short answer.** Yes, macOS 27 can be brought up on this PC, but only as a **fully emulated "virtual Mac"**. QEMU's binary translator (TCG) converts every arm64 instruction, kernel and apps alike, into x86_64 code at run time. That is the "translation" in your question, applied to the whole system. It will work, but it will be slow.

It cannot run natively on the PC the way a Hackintosh does. KVM cannot speed it up either, because KVM only accelerates guests of the host's own CPU architecture (xlat-01).

**Why it is feasible.**
- **Apple still ships a virtual-Mac target.** The macOS 27.0 and 27.0.1 IPSWs contain VirtualMac2,1, the kind of Mac that Apple's Virtualization.framework runs on Apple Silicon (vm-01). Its kernel needs no Secure Enclave, no SPTM/TXM page-table monitors and no GXF. It uses only 13 Apple-specific CPU registers, all on optional paths (vm-02, vm-06, xlat-03, xlat-07). Every physical Mac target, even M1, needs all of those components (xlat-04).
- **QEMU already models most of the hardware.** That covers the memory map, interrupt controller, serial port, firmware block device, disks and crypto engine (vm-09, vm-11).
- **It has been done for macOS 13.** Two projects boot macOS 13 this way on x86 Linux:
  - "27on86" runs headless (proj-06).
  - "orchard" reaches a GPU-rendered desktop on AMD's RADV driver, the same driver family as your GPU (proj-07).

  Nobody has published a macOS 14 to 27 boot by this route yet (boot-32). One unverified user report says macOS 27.0 reached its installer environment (proj-08).
- **We don't need a Mac to make a disk.** A pre-installed macOS 27.0 virtual-Mac disk is publicly downloadable as Cirrus Labs' Tart image (proj-28, boot-31).

**What we must build (all understood, none trivial):**
- **A hypercall service for pointer-authentication (PAC) keys.** Without it, every macOS 14.1+ guest spins forever before printing anything (vm-13, proj-05).
- **Stubs for three new paravirtual devices** (vm-18, vm-19, vm-20).
- **An MSI interrupt frame** (vm-22).
- **A 24 MHz timer setting** (xlat-23 +fc).
- **A boot loader** of our own for debugging (boot-17, boot-18, boot-22).
- **A port of the open-source GPU bridge** (Reims: Apple paravirtual GPU to Vulkan) to macOS 27's protocol changes (gfx-08 to gfx-11).

**What it will feel like.** These are measurements on this CPU. Emulated code runs about 10 to 17× slower than native code for kernel-style integer work, about 29× slower for large memory copies and about 160× slower for vector floating point (xlat-12). Prior art boots macOS 13 to a desktop in 75 s to 4 min on comparable hosts (proj-30). My estimate for macOS 27: it boots in several minutes, and the desktop is usable for light work but not smooth.

**What is not realistic:**
- **Native ("Hackintosh") macOS 27.** An x86_64 kernel and x86_64 frameworks still ship (x86-08, x86-01). But Apple deleted the Intel platform drivers, and launchd, WindowServer, Finder, Dock and loginwindow are arm64-only (x86-10, x86-15). Combining the two is a multi-year research project (Track C below).
- **Emulating a physical M1 Mac with its GPU.** That is years of work, with no workable GPU path (Track B).
- **Darling-style user-mode emulation.** There is no arm64-to-x86 translation and no desktop (Track D).
- **Apple Account/iCloud, DRM (FairPlay) and probably Rosetta inside the guest.** These are expected to fail. The VM identity is bound to a real Mac's Secure Enclave (boot-10), and the Apple-private virtual devices for them don't exist outside Apple's hypervisor (gfx-24).

**Effort (my judgement, one experienced engineer):**

| Goal | Estimate |
|---|---|
| Headless macOS 27 with SSH | about 2 to 4.5 person-months |
| Graphical desktop | +2 to 4 |
| Polish, plus our own install from Apple's IPSW without an Apple-made image | +2.5 to 7, uncertain |

**Blocking right now: the host [S].**

| Resource | State | Need |
|---|---|---|
| Disk (`/home`) | About 14 to 26 GB free (97% full), swinging by gigabytes as other workloads write | The macOS 27 disk image alone needs about 36 GB |
| RAM | About 12 GiB available, 19 GiB of swap already used | Comfortable for headless work |
| VRAM | llama.cpp holds 12.9 of 16 GiB | GPU-desktop work needs most of it |

Everything up to M2 (kernel and ramdisk shell) fits today. M3 and later need space freed (Q2), and the GUI phase needs RAM and VRAM (Q3).

**Recommendation.** Take Track A: VirtualMac2,1 under QEMU TCG, in seven milestones (§4).
1. Boot the kernel directly with a loader we control (M1, M2).
2. Run Apple's real boot chain on the Tart 27.0 disk, headless with SSH (M3).
3. GPU desktop (M4).
4. Performance and usability (M5).
5. A Mac-free install path (M6).

Track C stays a parked research option.

Legal: Apple's licence allows macOS virtualization only on Apple-branded hardware. This plan treats the work as private interoperability research. It never redistributes Apple binaries; we ship only our own code and patches, and users supply their own IPSWs and images.

---

## 2. Load-bearing facts

### 2.1 Facts table

**A. Target and CPU contract**

| # | Fact | Status | Source |
|---|---|---|---|
| F1 | macOS 27.0.1 is build 26A434, but its kernel is "Darwin Kernel Version 27.0.0 … xnu-13432.1.9~1/RELEASE_ARM64_VMAPPLE". It is Darwin 27, not 26. | confirmed | vm-05, x86-09; `recon/27.0.1/vma2/kernel_version.txt` |
| F2 | The 27.0.1 IPSW has exactly one virtual board: VirtualMac2,1 / vma2macosap (CPID 0xFE00, BDID 0x20, SDOM 1, platform vmapple2). | confirmed | vm-01, boot-01; `recon/27.0.1/BuildManifest.plist` |
| F3 | The VM identity carries no SEP, SPTM, TXM, ExclaveOS, GPU or ANE firmware. All 59 physical-Mac device classes, M1 included, require SPTM and TXM. | confirmed | vm-02, xlat-04 |
| F4 | The VMAPPLE kernelcache is an arm64e MH_FILESET with a single `__TEXT_EXEC`, no PPL/SPTM, and zero GXF genter/gexit instructions. | confirmed | vm-06, xlat-03 |
| F5 | The Apple-specific CPU surface is 13 IMPDEF registers (56 static uses), all on gated or rare paths. The Mac14,2 kernel uses 183 registers and 174 SPTM entries. The IMPDEF and hypercall sets are identical in 26.6.2 and 27.0.1. | corrected / confirmed | xlat-03, xlat-07, xlat-24 |
| F6 | Every architectural system register and SYS op the VM kernel uses executes under QEMU 11.1.2 `-cpu max`. Only some debug-register indices UNDEF, and XNU bounds-checks those. | corrected | xlat-08; `recon/factcheck-xlat/sysreg_probe_max.txt` |
| F7 | PAC is paravirtualized. The kernel never writes PAC key registers; LLB and mBoot program them, and the kernel calls HVC 0xC1000000 to 0xC1000006 (plus NOP 0xC10000F0). | confirmed / corrected | xlat-05, vm-14; XNU `osfmk/arm64/hv_hvc.h` (xnu-12377.121.6) |
| F8 | Since xnu-10002.41.9 (macOS 14.1), these calls are followed by `cbnz x0, .`, an infinite loop on failure. 27.0.1 has 19 such sites, the first before the MMU is enabled. Stock QEMU returns −1, so the guest hangs silently. | confirmed / corrected | vm-13, proj-05; `recon/vmapple-diff/isa/hvc_spin_on_failure.tsv` |
| F9 | The PAC contract that works (orchard, macOS 13): real architected PAC (a no-op PAC panics with "JOP Hash Mismatch"); boot-CPU keys installed on secondary cores at SET_INITIAL_STATE; non-zero GET_DEFAULT_KEYS (otherwise launchd dies with SIGBUS). | confirmed (+fc) | xlat-11, xlat (missed); `recon/prior-art/others/orchard/TECHNICAL.md` |
| F10 | XNU takes its timebase from the device tree (24 MHz) and ignores CNTFRQ_EL0. QEMU `-cpu max` defaults to 1 GHz, so we must set `cntfrq=24000000`. | confirmed (+fc) | xlat-23; xnu `pexpert/arm/pe_identify_machine.c`; QEMU `target/arm/internals.h:88` |

**B. QEMU and the device model**

| # | Fact | Status | Source |
|---|---|---|---|
| F11 | An x86 host has no hardware acceleration (KVM, HVF, WHPX) for arm64 guests. The locally built `qemu-system-aarch64` 11.1.2 offers only `tcg`. | confirmed | xlat-01; `tools/qemu-src/build/qemu-system-aarch64` |
| F12 | Upstream QEMU's vmapple machine needs a macOS host. Its Kconfig says `depends on HVF`, the default CPU is `host`, and `CONFIG_VMAPPLE=n` since 2025-04-03. The docs say macOS 12 guests only, and apple-gfx wraps macOS's ParavirtualizedGraphics.framework. | confirmed | vm-10, proj-01, xlat-02, gfx-03 |
| F13 | QEMU's vmapple memory map and IRQs match the vma2 device tree except the PCIe MMIO window (QEMU 0x1fff0000 vs DT 0x2fff0000). Nothing is modelled at 0x1fff0000 (MSI frame), 0x30240000 (avp-rtc), 0x30260000 (avp-ctrr) or 0x30270000 (avp-sealed-registers). | corrected | vm-09, vm-27 |
| F14 | Device-tree history: 12.6 to 13.6 have identical devices. 14.0 adds avp-rtc and the GIC MSI frame, 26.0 adds avp-ctrr, 27.0 adds avp-sealed-registers. | confirmed | vm-08; `recon/vmapple-diff/devicetree_diff_all_versions.txt` |
| F15 | avp-rtc is optional: mBoot falls back to PL031 unless 0x30240040 reads 0xF001. But under TCG something must be mapped there, because unassigned MMIO raises an external abort. | confirmed | vm-19 |
| F16 | avp-ctrr must read as zero (graceful fallback) or be fully consistent. A partially emulated device makes mBoot panic. | corrected | vm-18 |
| F17 | avp-sealed-registers is skipped while bit 0 of cfg+0x6ec is clear, which it is in QEMU's zeroed cfg page. | confirmed | vm-20 |
| F18 | Newer guests need a GICv2m-style MSI frame. Mediouni reports an early kernel panic without the MSI_TYPER bit-31 quirk; Peter Maydell objected, and upstream has not resolved it. | confirmed | proj-03, vm-22 |
| F19 | The guest has no virtio-gpu driver. Its only display and GPU path is Apple's paravirtual GPU (PVG) at 0x30200000 / 0x30210000. | confirmed | gfx-01, gfx-04, vm-23 |

**C. Boot and disk**

| # | Fact | Status | Source |
|---|---|---|---|
| F20 | AVPBooter.vmapple2.bin, the VM boot ROM, is on the IPSW's encrypted system volume, and Apple serves the volume key without authentication. **[S]** Extracted from 27.0.1 on Linux: 334,960 B, "AVPBooter for vmapple2", mBoot-20457.1.29, sha256 `6d0105…a845`. | corrected / confirmed + [S] | vm-11, boot-04, boot-05; `recon/27.0.1/avpbooter/` |
| F21 | **[S]** orchard's AVPBooter patch (first two instructions become `mov x0,#0; ret`) expects offset 0x2314, which fits macOS 13/14-era ROMs. In the 27.0.1 ROM, 0x2314 holds other code. The Image4 property-validation routine, which builds the AMNM/BNCH/BORD/CEPO/CHIP/CPRO/CSEC/SDOM/ECID/DGST tags, now starts at **0x2370** with exactly the expected prologue. | new; static analysis only | `recon/27.0.1/avpbooter/patch-site-check.txt`; `scripts/avpbooter_find_cepo.py` |
| F22 | Direct-boot ABI: `boot_args` revision 3 (1024-byte command line); the kernel is entered with the MMU off and x0 = physical `boot_args`; the entry point is 0xfffffe0009eb4480 (= IM4P `kcep`). | confirmed | boot-18 |
| F23 | Without iBoot, we set SIP and debug ourselves through the device tree (`/chosen/asmb` lp-sip0/lp-sip1, `/chosen/debug-enabled`), and the static trust cache through `/chosen/memory-map` TrustCache. The kernel does not re-verify the trust cache. | confirmed | boot-17, boot-22 |
| F24 | The erase restore ramdisk (`043-70833-687.dmg`, 373 MB, already local) is an unencrypted APFS volume. It contains bash, newfs_apfs, asr and the apfs_* tools, and launchd starts restored_external on `/dev/console`. | confirmed | boot-30; `downloads/ramdisk-27.0.1/` |
| F25 | A public pre-installed 27.0 VM exists: `ghcr.io/cirruslabs/macos-golden-gate-vanilla:27.0`. It has 67 disk layers (30,867,739,355 B compressed), expanding to a 35,735,707,648 B ASIF container that holds a 50 GB disk. The aux (NVRAM) layer is 33,579,164 B, the ECID is 0xb484f40bb3096b80 and the hardware model is DataRepresentationVersion 2. **[S]** Its build template sets user `admin`/`admin`, Packer logs in over SSH, and sudo is passwordless. | corrected / confirmed + [S] | proj-28, boot-31, boot-11; `recon/27.0/tart/manifest-vanilla-27.0.json`; cirruslabs/macos-image-templates `templates/vanilla-golden-gate.pkr.hcl` |
| F26 | ASIF and the Apple-LZ4 layers can be decoded on Linux: dissect.hypervisor (AGPL-3.0), the CC0 spec huven/asif-format, and `scripts/applelz4.py` (verified against the layer digest). QEMU has no ASIF driver. | corrected / confirmed | proj-28, boot-31 |
| F27 | The Data and Preboot volumes of a VZ guest are not encrypted (APFS fs_flags 0x1; DT `apfs-crypto-disabled=1`). | confirmed (+fc) | boot (missed); `scripts/asif_scan_apsb.py` |
| F28 | Linux cannot create a bootable macOS APFS layout. mkapfs makes a single volume. linux-apfs-rw refuses writes to sealed volumes and is a kernel module, so it needs root. | confirmed | boot-29 |
| F29 | A VM's LocalPolicy is signed inside the guest with an Apple test key embedded in the release kernel (AppleVPBootPolicy), and its nonces live in NVRAM. Minting our own policy offline looks possible but is untested. | confirmed (corrected detail) | boot-15, boot-16; `recon/factcheck-boot/lpol-signature-chain.txt` |
| F30 | The "HostKeys" trailer in the aux file (macOS 15+) is an identity bound to the host Mac's Secure Enclave and probably cannot be reproduced off Apple hardware. | corrected | boot-10 |
| F31 | Third parties report that Apple's signing server issued VirtualMac tickets to Linux tools (13.6 and 26.5.2). I did not probe it: the permission system blocked the request. A Linux DFU restore reached a personalized iBSS upload and then stalled. | confirmed (medium) | boot-13, boot-14, proj-31 |

**D. Graphics**

| # | Fact | Status | Source |
|---|---|---|---|
| F32 | The 27.0.1 PVG driver proposes protocol 252 and walks down to a version it knows. Rung 62, Reims' maximum, is valid. A stub that reads 0 makes the driver back out cleanly ("host doesn't appear to be configured with GPU"). | corrected | gfx-08, gfx-09 |
| F33 | With no Metal device, WindowServer aborts about once a minute on macOS 26, while SSH and the command line keep working. Not verified on 27. | confirmed (26) | gfx-13 |
| F34 | Reims (PVG to Vulkan) is proven in two setups: arm64 macOS 13 on x86 Linux under TCG on RADV (orchard), and macOS 26 under KVM on Asahi (aquarat's fork). Upstream Reims, idle since 2026-09-03, lacks the macOS 26+ IOSurface-mapper register layout; aquarat's fork has it. 27.0.1 adds a non-page-aligned IOSurface offset (+0x60). | confirmed / corrected | proj-07, gfx-12, gfx-10, gfx-11 |
| F35 | The host GPU is an RX 7600 XT (RADV NAVI33, Mesa 26.2.4, Vulkan 1.4). It has the Vulkan features Reims needs, but no ASTC. | confirmed | gfx-32; `recon/gfx/vulkaninfo-host.txt` |

**E. Performance on this host**

| # | Fact | Status | Source |
|---|---|---|---|
| F36 | TCG vs native on the same C code, system-mode geometric mean: 10.0× (4K pages), 11.2× (16K pages), 13.2× (16K pages + impdef PAC), 16.7× (QARMA5 PAC). By workload: function calls 16×, indirect calls 25×, scalar FP 10×, NEON FP64 159×, 1 GB copy 29×. | confirmed | xlat-12; `recon/xlat/bench/KEY_RESULTS.txt` |
| F37 | PAC instructions are 6.27% of the VM kernel's static instructions. impdef PAC costs 4.4 to 4.9× on call-heavy code; QARMA5 costs 40 to 44×. | confirmed / corrected | xlat-10, xlat-11 |
| F38 | Multi-threaded TCG scales about 5.9× at 6 vCPUs on a CPU-bound test; orchard found 12 vCPUs fastest for a macOS 13 boot. QEMU treats 16K guest pages as "large pages", so any page invalidation flushes that whole soft-TLB. | confirmed | xlat-15, xlat-17 |
| F39 | Reference boots: orchard's macOS 13 reaches the desktop in about 75 to 105 s with 12 vCPUs (238 s in another configuration; 2 to 3 min in a user's report). 27on86's 8-vCPU headless validation cycle takes 492 to 524 s. | confirmed | proj-30 |

**F. x86_64 code left in macOS 27 (Track C)**

| # | Fact | Status | Source |
|---|---|---|---|
| F40 | The 27.0.1 system volume ships a full x86_64 kernel (RELEASE_X86_64, Darwin 27.0.0), and the KDK adds DEVELOPMENT and KASAN x86_64 kernels. No x86_64 kernel collections ship. | confirmed / corrected | x86-08, x86-09, x86-13 |
| F41 | 27 removed the core Intel platform kexts (ACPIPlatform, APIC, HPET, EFI runtime/NVRAM, SMBIOS, AppleHV, AMD/Intel GPU drivers). Sandbox.kext has no x86_64 slice. | confirmed | x86-15, x86-14 (corrected) |
| F42 | The new RosettaOS cryptex holds macOS 27's x86_64 dyld shared caches: about 6.77 GB main, plus 1.5 GB "x86Support" including AppKit and SkyLight. | confirmed / corrected | x86-01, x86-02, xlat-25 |
| F43 | In 27, launchd, xpcproxy, WindowServer, loginwindow, Finder, Dock, logd, amfid, cryptexd, kernelmanagerd and sandboxd are arm64e (and arm64e.x1) only. | confirmed | x86-10 (+fc) |
| F44 | The latest Tahoe release is 26.7.1 (25G241). The newest Tahoe IPSW is 26.6.2. | confirmed | x86-20 |

**G. Host and legal**

| # | Fact | Status | Source |
|---|---|---|---|
| F45 | **Disk:** `/home` had 14.3 to 17.2 GB free (97% used) between 23:40 and 23:47 CEST (one brief dip to 3.5 GB) and 26.0 GB at 00:01 on 2026-10-11; an EA Desktop (Wine) process was writing about 39 MB/s. **Memory:** RAM 31 GiB, 12.9 GiB available, 19.2 GiB swap in use; the Windows VM has `-m 8G -smp 2` (3.4 GB resident, 5.0 GB swapped); VRAM is 12.9 of 16 GiB used, by llama.cpp. **Other:** THP is set to `always`; `/dev/kvm` and `/dev/vhost-vsock` are usable. | [S] | `df`, `free`, `/proc`, sysfs |
| F46 | Apple's licence permits macOS virtualization only on Apple-branded hardware. | confirmed (as quoted) | boot-35 (orchard README quote); proj-23 (security-pcc licence §1) |
| F47 | "The QEMU emulator as a whole is released under the GNU General Public License, version 2." | [S] | `tools/qemu-src/qemu-11.1.2/LICENSE` |

### 2.2 Unverifiable, self-reported or unchecked (do not plan on these)

- **U1.** Rian-AP's macOS 27.0 restore-environment boot under TCG on Windows is self-reported, and the patches are not public (proj-08).
- **U2.** The NextCore, hackmate-goldengate and MaliosDark claims are self-reported with no reproducible code (proj-13, proj-14).
- **U3.** Whether Apple's signing server signs VMA2 builds today is unknown. It was not probed (boot-13).
- **U4.** It is unknown whether the Cirrus Labs 27.0 image boots under QEMU at all. Three things could stop it: the DataRepresentationVersion 2 hardware model, keybags wrapped with VZ's hardware AES keys, and the HostKeys trailer. No project documents a v2-descriptor image under QEMU (proj, missed); orchard's v1 Ventura image works with QEMU's dummy AES keys.
- **U5.** Whether iBoot accepts an offline-minted LocalPolicy is untested (boot-15, boot-16).
- **U6.** Four guest-behaviour questions are unverified on 27:
  - whether WindowServer still aborts without Metal (gfx-13);
  - whether the AppleVirtIO used-ring bug persists (gfx-25);
  - whether QEMU virtio-input binds (gfx-22);
  - whether AppleQEMUGuestAgent speaks over vsock port 505050 under QEMU (gfx-29).
- **U7.** The 27.0.1 AVPBooter patch site 0x2370 is a static identification only (F21). It has not been run.
- **U8.** Every effort estimate and every macOS 27 boot-time expectation on this CPU is judgement (xlat-27 is marked unverifiable).
- **U9.** The 27.0.1 IPSW `wget` process exited around 23:43 CEST. Its size (26,637,307,067 B) and CRCs were not checked here; the directory was deliberately not touched.
- **U10.** The image list of the main x86_64 dyld cache is unchecked (x86-03). This matters for Track C only.
- **U11.** The licence-compatibility notes in §9 are not legal advice.
- **U12.** Whether Rosetta works inside the emulated guest is unknown (proj, open question).

### 2.3 Corrections to the brief and superseded statements

| Earlier statement | Use instead |
|---|---|
| "macOS 27 = Darwin 26" (brief) | Darwin 27.0.0, xnu-13432.1.9 (F1) |
| "124 GB free on /home" (brief) | About 14 to 26 GB free and fluctuating (F45) |
| "RX 7600" (brief) | RX 7600 XT, 16 GB, mostly occupied by llama.cpp (F35, F45) |
| "qemu-system-aarch64 not installed" (brief) | Still true system-wide, but a TCG-only 11.1.2 build is at `tools/qemu-src/build/` (F11) |
| "Latest Tahoe is 26.6.2" (brief) | 26.7.1 (25G241); 26.6.2 is only the newest Tahoe IPSW (F44) |
| "QEMU's vmapple memmap matches the DT exactly" | It matches except the PCIe MMIO window (vm-09) |
| "AVPBooter is not in any IPSW" | It is on the AEA-encrypted system volume, and the key is public (vm-11, F20) |
| "No public ASIF parser exists" | dissect.hypervisor and a CC0 spec exist (proj-28) |
| "The PAC spin-loop starts with macOS 15" | It starts with 14.1 per the XNU source (proj-05); binaries show none in 14.0 and spins from 15.0 on |
| "A mismatched avp-ctrr just falls back" | A partial match panics mBoot (vm-18) |
| "orchard's AVPBooter patch is at 0x2314" | True for 13/14-era ROMs only; the 27.0.1 ROM needs 0x2370 (F21) |
| "Upstream Reims' mapper works on macOS 26" | It does not; aquarat's fork is needed (gfx-10, gfx-11) |
| "A null/no-op PAC is a cheap speed-up" | A no-op PAC panics; PAC must be self-consistent (xlat-11) |
| "Inferno is GPL-2.0; m1n1 is MIT" | Inferno's own code is AGPL-3.0; m1n1 is GPL-2.0-or-later since 2.0.0 (boot, missed) |

---

## 3. Track comparison

| | A. VirtualMac2,1 in QEMU TCG | B. Physical Apple-Silicon emulation (Inferno-style) | C. "Reverse Rosetta" hybrid | D. User-mode only (Darling-like) |
|---|---|---|---|---|
| **Idea** | Emulate Apple's own virtual Mac (the board VZ exposes) and run the stock OS on it | Emulate a real M1 (t8103) SoC and boot the physical-Mac kernelcache | x86_64 XNU (27's own, or 26.x) under KVM, plus 27 userland; translate only the arm64e-only processes | No kernel; run Mach-O binaries on Linux with syscall emulation and per-process translation |
| **What can boot** | macOS 13 already does on x86 Linux, headless and to a desktop (F39). For 27 the CPU contract is small and stable (F5 to F9); the device gaps are known (F13 to F18) | physical 27.0 kernels reach a ramdisk root shell: single core, PAC disabled, no storage or display (proj-12) | unknown. The kernel exists (F40), but platform kexts, Sandbox and the key daemons are missing or arm64-only (F41, F43) | individual CLI tools at best (proj-21, xlat-20) |
| **Performance on the 5600X** | each vCPU gives roughly 1/10 to 1/17 of a native core on integer work and about 1/160 on FP64 vector code (F36); 4 to 8 vCPUs; boot in minutes (est.) | worse than A: today single-core, plus SPTM/GXF emulation | native for x86_64 code; translated processes at user-mode JIT speed (qemu-user measured 6.5 to 8× geomean, F36 / xlat-12) | 6.5 to 8× for translated code; no system services |
| **GUI / GPU** | PVG is translated to Vulkan by Reims on RADV; proven for 13 (TCG) and 26 (KVM); needs the 27 protocol deltas (F32, F34) | none realistic: no AGX GPU model, DCP display firmware needed; software-render patches shown only for iOS 14 (gfx-19) | very poor: WindowServer is arm64-only, there is no macOS driver for the RX 7600, and the x86 AppleParavirtGPU was removed in 27 (x86-15) | none |
| **Effort** | headless 2 to 4.5 PM; +2 to 4 PM GUI; +2.5 to 7 PM polish and own install (est.) | 1 to 3 PM to reproduce darwin-vm's shell; ≥24 PM for a desktop (est.) | 2 to 4 PM spike to a CLI shell (likely to fail); ≥24 to 36 PM for a GUI hybrid (est.) | 6 to 12 PM for a CLI prototype (xlat-20, est.) |
| **Verdict** | **Recommended** | Reject (keep as a reference) | Park (optional spike R1) | Reject |

PM = person-months; "est." = judgement (U8).

**Track A, main risks.**
- macOS 27-specific unknowns: sealed registers, identity/provisioning virtio devices 0x1a0c and 0x1a11, HostKeys (F17, F30, gfx-24).
- TCG speed (F36).
- PVG protocol 71.x deltas (gfx-05, gfx-10).
- Image provenance (U4).
- The licence (F46).

**Track A, reuse.**
- yaelliethy/orchard: TCG vmapple machine, `hvc.c` PAC service, GICv2m quirk, bdif NVRAM writes, cfg seed, Tart and AVPBooter scripts.
- aquarat/qemu-reims-vgpu and aquarat/reims-vgpu: avp-rtc, the virtio used-ring fix, macOS 26 mapper layout, Linux page views.
- steelbrain 27on86: headless stubs, boot-argument injector.
- DenrianWeiss/w515y: AES fixes.
- Mediouni RFC v8: GICv2m, FileVault-off virtio-blk capability.
- steelbrain/metal2vulkan.
- AsahiLinux/m1n1 (GPL-2.0): direct-boot reference.
- blacktop/ipsw, dissect.apfs and dissect.hypervisor, and our own `scripts/`.

**Track B.**
- **Risks:** every physical 27 identity needs SPTM and TXM (and ExclaveOS on newer SoCs) (F3); the M1-class kernel uses 183 IMPDEF registers and 174 SPTM calls (F5); the device surface is huge (AIC, DART, ANS NVMe, SMC, PMGR, DCP, AGX); Inferno's code is AGPL.
- **Reuse:** jprx/darwin-vm and qemu-sptm, ChefKiss Inferno, the archived qemu-t8030.

**Track C.**
- **Risks:**
  - It would load 26.x platform kexts and Sandbox into a Darwin 27 kernel, crossing KPI and private-KPI boundaries (x86-15, x86-16).
  - kmutil is arm64-only in 27, so building an x86 KC needs a 26.x x86 environment.
  - The kernel ABI drifted: 8 new syscalls and changed MIG subsystems (x86-16).
  - launchd itself would need translation (F43).
  - No xnu-13432 source has been published yet (x86-18).
  - Apple can remove the remaining x86 code at any time.
- **Reuse:** OpenCore, OSX-KVM, blacktop/darwin-xnu-build, the 27.0.1 KDK, RosettaOS caches, QEMU linux-user or dynarmic as the JIT, and Darling's Mach-IPC ideas.

**Track D.**
- **Risks:** QEMU darwin-user was dropped in 2012, and Darling is x86_64-only with an unmerged ARM port (xlat-21, proj-21).
- **Verdict:** a niche tool, not a way to "bring up macOS".

---

## 4. Recommended architecture and milestones

### 4.1 Architecture

```
Linux host (CachyOS, Ryzen 5 5600X, no arm64 acceleration)
└── gg-qemu  = qemu-system-aarch64, TCG with multi-threaded TCG (MTTCG)
    │          base: orchard tree (QEMU 11.1.50) + attributed patch queue + our macOS 27 work
    ├── machine "vmapple" (TCG): memory map of the vma2 DT (F13)
    │   ├── CPU: apple-vm-cpu = max + pauth-impdef + cntfrq 24 MHz (F9, F10)
    │   ├── HVC service: PAC 0xC1xxxxxx | SMCCC UID/OEM 0x83xxxxxx/0xC3xxxxxx | PSCI | 0xC6 → NOT_SUPPORTED
    │   ├── GICv3 + GICv2m MSI frame @0x1fff0000 (MSI_TYPER bit-31 quirk)
    │   ├── PL011 · PL031 · PL061 · pvpanic · cfg page · bdif(aux, root) · AES(GnuTLS)
    │   ├── avp-rtc · RAZ/WI windows: avp-ctrr, avp-sealed-registers
    │   ├── PCIe: vmapple-virtio-blk (root/aux), virtio-net, qemu-xhci(kbd, tablet), virtio-sound/vsock/fs…
    │   └── PVG: inert stubs (M1–M3) ──► reims-vgpu-mmio (M4) ──► Vulkan / RADV / RX 7600 XT
    ├── Boot mode 1 "ggboot" (debug/rescue): our trampoline + ADT + boot_args r3 + trust cache + ramdisk
    └── Boot mode 2 "stock":  AVPBooter (patched) → LLB (aux) → mBoot/iBoot → XNU 27 (disk)
Guest: unmodified macOS 27 VirtualMac2,1 (arm64e) from the Tart 27.0 image, later our own install
```

**Why Track A.**
1. It has the smallest Apple-specific CPU contract of any macOS 27 target (F3 to F5). That contract did not change between 26.6.2 and 27.0.1 (xlat-24), so it is likely to survive 27.x updates.
2. Most of the board already exists in QEMU (F13), and the rest is known and small (§5).
3. A GPU path exists that needs no Apple host framework (F34).
4. Prior art proves the full stack on the same host class: x86 Linux, TCG and RADV (F39).
5. It does not depend on the Intel kexts and userland that Apple is removing (F41, F43).

The price is TCG speed (F36).

**Design decisions.**
- **D1. Emulate Apple's virtual Mac, never a physical one** (F3 to F5).
- **D2. Base and licensing.** Start from orchard's QEMU tree, pinned at `c73a2bda1fd4` (2026-09-25), because it is the closest prior art. Carry our changes as an attributed, rebase-able patch queue against upstream master. Keep code inside the QEMU tree GPL-2.0-or-later, and never paste AGPL code (Inferno, dissect) into it (F47).
- **D3. Two boot modes.**
  - **ggboot (direct):** total control over SIP, boot arguments and trust cache, with no fight with LocalPolicy or tickets (F22, F23). It needs no disk image, so it fits today's disk.
  - **stock chain:** a correctly sealed system that can later receive updates.
- **D4. Headless first.** PVG stubs make AppleParavirtGPU back out cleanly (F32). The GUI comes only after a stable headless system.
- **D5. Disk image order.** Use the pre-installed disk first (Tart 27.0); make our own install last (M6).
- **D6. Keep the guest OS unmodified.** All adaptation happens on the QEMU side, so the sealed system volume, LocalPolicy and updates stay valid.
- **D7. Performance comes from QEMU-side changes**, measured with `scripts/xlatbench/`: MTTCG, impdef PAC, a 2 GiB TB cache, 16K-TLB work, WFK/KICK hypercalls for idle vCPUs.
- **D8. Hygiene.**
  - The host never executes downloaded binaries; Python that parses downloads runs with `-I`.
  - Third-party forks and crates are reviewed and pinned (`cargo --locked`), and QEMU runs unprivileged.
  - Rust uses `RUSTUP_TOOLCHAIN=stable`, so no global rustup change is needed.

### 4.2 Milestones

| ID | Milestone | Deliverable | Exit criteria | Effort (est.) | Depends on |
|---|---|---|---|---|---|
| M0 | Foundations | `gg-qemu` source plus a reproducible build; patch queue; test harness; extracted 27.0.1 artifacts | builds `qemu-system-aarch64` and `qemu-img`; `-M vmapple -accel tcg` starts on x86; the patched 27.0.1 AVPBooter runs past 0x102370 and issues bdif aux reads or waits in DFU (gdb/trace) | 1 to 2 PW | ≥ 10 GB free disk (14 to 26 GB now) |
| M1 | Kernel banner via direct boot | `scripts/ggboot/`: ADT builder, Mach-O fileset layout, `boot_args` r3, trampoline ROM; HVC service; RAZ stubs | the PL011 log shows "Darwin Kernel Version 27.0.0 … RELEASE_ARM64_VMAPPLE" and early IOKit/kext start messages on 1 vCPU; no unassigned-MMIO aborts; 3/3 runs identical | 2 to 4 PW | M0 |
| M2 | Rescue shell on the restore ramdisk | direct boot with md0 root, SMP, real PAC; interactive bash on serial | `uname -v` shows xnu-13432.1.9; `sysctl hw.ncpu` ≥ 4; no JOP or PAC panics in 3 boots; a virtio scratch disk is visible; boot time recorded | 2 to 4 PW | M1 |
| M3 | Real boot chain, headless macOS 27.0 | the Tart 27.0 disk boots AVPBooter → LLB → mBoot → XNU → launchd → sshd under gg-qemu | `ssh admin@localhost -p 2222 sw_vers` reports 27.0 / 26A428; 3 cold boots in a row to SSH; no kernel panics (a WindowServer crash loop is acceptable); time-to-SSH recorded | 3 to 8 PW | M0 (+M1 learnings); ≥ 45 GB free (Q2) |
| M4 | Graphical desktop | aquarat's reims-vgpu fork and metal2vulkan integrated, with macOS 27 deltas; display in the Reims window or VNC; USB keyboard and tablet | login window and Finder desktop render; `system_profiler SPDisplaysDataType` lists the Apple Paravirtual device; Safari and TextEdit open; 30 minutes with no GPU panic or hang | 8 to 16 PW | M3; RAM and VRAM (Q3) |
| M5 | Usability and performance | tuned launcher; perf report; audio, shared folder, control channel, clipboard; snapshot workflow | boot time and UI latency measurably better than the M3/M4 baselines; audio out, shared folder and host↔guest command channel work | 4 to 12 PW | M3 (GUI items: M4) |
| M6 | Mac-free provisioning and updates | a new VM with its own ECID, made on Linux from Apple's IPSW only, or a documented negative result | boots to Setup Assistant or SSH with no Apple-hardware-made artifact; or a written analysis of the blocking step | 6 to 16 PW (high uncertainty) | M2 + M3; network approval (Q6) |

PW = person-weeks.

**Cumulative estimates:** M0 to M3 is about 8 to 18 PW (2 to 4.5 PM); through M4 about 4 to 8.5 PM; everything about 6.5 to 15.5 PM. The anchors are prior art reaching macOS 13 with heavy reuse, plus the 27-specific unknowns (U8).

#### M0: Foundations (1 to 2 PW)
- **Disk (Q2).** M0 to M2 fit in the current free space; freeing about 45 GB is needed only before M3. Verify or delete the 27.0.1 IPSW: its wget has exited, and it is only needed for M6 (U9).
- **Get the code.** Shallow-clone orchard at `c73a2bda1fd4`. Review its diff against upstream before building it: this is supply-chain review, since building and running it executes its code on the host.
- **Build.** Configure TCG, GnuTLS crypto (already detected locally, F11), slirp and tools (`qemu-img`, which is missing system-wide). Add GTK/VNC/OpenGL later, for M4.
- **Import the patch queue,** each patch with its source and licence:
  - aquarat: `hw/vmapple/avp-rtc.c`, `x-fix-overlapping-used`;
  - w515y: AES fixes 0004 and 0005;
  - Mediouni v8 10/10: the virtio-blk FileVault-off capability.
- **Harness.**
  - Serial log capture.
  - `-d guest_errors,unimp` plus tracing of unassigned MMIO.
  - gdbstub scripts (system gdb 18.1 handles aarch64).
  - Timing.
  - `scripts/xlatbench` re-runs for performance regressions.
- **ROM smoke test.**
  - Patch the 27.0.1 AVPBooter at 0x2370 with our own script or orchard's `--offset 0x2370`.
  - Run it with no aux and break after the routine.
  - Expect bdif reads of the aux device, or a wait in DFU. This tests ROM/TCG compatibility before any disk work.
- **Optional, only with 25 GB or more spare:** reproduce orchard's macOS 13 Ventura boot (22.58 GB download) as a known-good baseline and host timing reference.

#### M1: Kernel banner via direct boot (2 to 4 PW)
- **Kernel layout.** Reverse `lay_out_opaque_kernelcache` in our local mBoot image (`recon/27.0.1/vma2/iBoot.vma2.RELEASE.raw`; it uses IM4P kclo/kcrz/kcwf/kcwz/kclf/kclz, boot-18). Place the fileset segments at physBase; derive virtBase and physBase.
- **ADT builder.** Start from the IPSW template (`DeviceTree.vma2macosap.raw`; ADT format per boot-19):
  - Strip placeholder bits.
  - Fill `/chosen`: memory-map entries DeviceTree, BootArgs, TrustCache, RAMDisk, BootKC-*; `random-seed`; `boot-nonce`; a valid empty `nvram-proxy-data`; `chip-id 0xFE00`; `board-id 0x20`; `unique-chip-id`; `security-domain 1`; `asmb` lp-sip0/lp-sip1; `debug-enabled 1`.
  - Keep N cpu nodes.
  - Fill `/memory`.
- **Boot arguments.** Build `boot_args` revision 3 with the command line `-v serial=3 debug=0x14c …`.
- **Trampoline ROM** (our own code, about 50 instructions, loaded as `-bios`): program the APIA/APIB/APDA/APDB/APGA keys, set x0 = `boot_args`, branch to the kernel entry.
- **HVC service:**
  - Port orchard's `hvc.c`: 0xC1 calls return 0; SET_INITIAL_STATE copies the boot CPU's keys; GET_DEFAULT_KEYS returns non-zero.
  - 0xC6xxxxxx returns NOT_SUPPORTED.
  - SMCCC UID, revision and features come later (M2/M5).
- **Machine setup:**
  - `cntfrq=24000000` (F10).
  - RAZ/WI windows at 0x30240000, 0x30260000 and 0x30270000.
  - pvpanic window widened to 0x10000 (vm-08).
  - Inert PVG stubs (F32).
  - The KDK's `kernel.release.vmapple` with its dSYM for symbols (check the UUID match). Range-extract it with `scripts/udif_remote.py` if there is no room for the 1.25 GB DMG.

#### M2: Rescue shell on the restore ramdisk (2 to 4 PW)
- **Root on the ramdisk.**
  - md0 root: the RAMDisk memory-map entry plus `rd=md0`.
  - Merge trust-cache module 0 from the static cache and the ramdisk cache, kept sorted (boot-22).
  - First target: restored_external writing to the console with the ramdisk unmodified (F24).
- **Interactive bash.** Re-point the restored_external launchd job at `/bin/bash`; bash is already in the ramdisk and its trust cache. Ways to edit the ramdisk, in order of preference:
  1. A same-length in-place edit of the plist data blocks. The volume is unsealed, and APFS does not checksum file data.
  2. A small helper Linux VM under KVM with linux-apfs-rw. That gives root inside the VM; there is no host sudo.
  3. Try launchd's single-user mode.
- **SMP and devices.**
  - SMP through PSCI CPU_ON and SET_INITIAL_STATE.
  - GICv2m MSI frame.
  - AES engine sanity check (AppleVPKeyStore must not hang, boot-34).
  - A virtio-blk scratch disk.
- **Deliverable.** This environment is also the M6 workbench: Apple's own newfs_apfs, asr and apfs_* tools run inside it.

#### M3: Real chain, headless (3 to 8 PW)
- **Disk image.** Stream-convert the Tart 27.0 layers (Apple LZ4 → ASIF → sparse raw), verifying the per-layer digests and the GPT layout (boot-26).
- **Aux, ECID and ROM.**
  - Build `aux.img`: strip the first 0x4000 bytes and keep the rest.
  - `-M vmapple,uuid=0xb484f40bb3096b80` (boot-11).
  - Use the patched AVPBooter (F21); prefer the one extracted from the image itself, which is version 27.0.
- **Boot arguments.**
  - Append to NVRAM, append-only, with the bank checksum recomputed (boot-16).
  - Or inject at the XNU handoff through gdbstub. aquarat's injector handles `boot_args` revision 3; 27on86's hard-codes revision 2 at 0xACA00000 (gfx-14, gfx-16).
- **Device fixes:**
  - bdif NVRAM writes (orchard).
  - The FileVault-off virtio-blk capability. The Tart volumes are unencrypted (F27), and Mediouni reports that newer FileVault-off guests do not boot without it (proj-03).
  - The used-ring fix.
  - The avp-rtc device.
  - cfg seed at 0x5c8, plus fields 0x584/0x5c4/0x648 (vm-21).
  - Check aquarat's BDIF 64 GiB size note (proj-09).
- **Expected unknowns.** Debug U4 first: DataRepresentationVersion 2, AES/keybag and HostKeys. Then the sealed-register doorbells and the identity/provisioning virtio devices (log any access to their PCI IDs).

#### M4: Graphical desktop (8 to 16 PW)
Detailed plan in §7.

#### M5: Usability and performance (4 to 12 PW)
- **vCPU sweep.** Try 4, 6, 8 and 12 vCPUs with `tb-size=2048`.
- **PAC fast path.** Optimize PAC with a self-consistent inline impdef path (F37).
- **16K-page TLB.** Fix the full flushes: variable-size TLB entries, or 14-bit target pages for a machine that is 16K-only (F38).
- **Idle vCPUs.** Implement WFK/KICK (0xC3000005/6) so idle vCPUs stop burning host CPU (xlat-06).
- **Devices:**
  - virtio-sound with aquarat's JACK/CHMAP patch;
  - virtio-fs or 9p;
  - AppleQEMUGuestAgent over vhost-vsock port 505050 (gfx-29);
  - clipboard through tart-guest-agent's vdagent plus QEMU `qemu-vdagent`;
  - qcow2 overlays for snapshots.

#### M6: Mac-free provisioning (6 to 16 PW, may not converge)
Ranked options are in §6.4. This milestone needs network access to Apple's signing server (Q6) and space for the IPSW (26.6 GB) plus a new disk (50 GB).

#### R1 (optional, off the critical path): Track C spike (2 to 4 PM if you approve it, Q7)
- **Exit:** the x86_64 Darwin 27 kernel from the 27.0.1 system volume boots to a single-user shell under KVM with OpenCore and 26.7.1 platform kexts (the InstallAssistant is already downloaded), or a documented negative result.
- **Not a route to a GUI.**

---

## 5. QEMU device-model worklist (from the VirtualMac2,1 recon)

| Device / guest node | Address (INTID) | In DT since | QEMU upstream | Plan | When |
|---|---|---|---|---|---|
| CPU | — | — | `-cpu max` exists (TCG; PAuth impdef by default since 10.0) | `apple-vm-cpu` subclass (orchard): pauth-impdef, `cntfrq=24000000`; keep SME/SME2 (VMAPPLE.h) | M1 |
| Apple IMPDEF registers (13, e.g. AMX_*, ASPSR, AFPCR, CYC_OVRD, ERR_STS) | — | 12.6 (AMX 26.x) | missing (UNDEF) | RAZ/WI storage stubs behind a property; leave `guest_amx_version` unset so AMX stays off (xlat-07) | M1 (log), M5 |
| PAC hypercalls 0xC1000000–06, 0xC10000F0 | HVC #0 | 12.6 (spin since 14.1) | missing: returns −1, so the guest hangs (F8) | port orchard `hvc.c` (keys to secondaries, non-zero default keys) | M1 |
| SMCCC UID/revision/features 0x8300FF01/FF03/FEFF; OEM 0xC3000003–06 | HVC | 13.0 | missing; −1 just disables features | UID 3B878185-AA62-4E1F-9DC9-D6799CBB6EBB, revision 1.0; MABS offset, boot-session UUID; WFK/KICK for idle | M2/M5 |
| Nested-virt service 0xC6000000, 0xC6000010–18 | HVC | 26.x | missing | NOT_SUPPORTED (never spins, vm-12) | M1 |
| PSCI (HVC conduit) | — | all | exists | check the reported version (Ventura's ApplePSCI rejected 1.3 under KVM; aquarat pinned 1.1) | M1 |
| GICv3 distributor/redistributor | 0x10000000 / 0x10010000 | all | exists | — | — |
| GIC MSI frame (msi-frame-index 2) | 0x1fff0000 | 14.0 | missing | `arm-gicv2m` with `macos-compat` (MSI_TYPER bit 31), wired to PCIe (orchard, aquarat, Mediouni) | M2/M3 |
| PL011 uart0 | 0x20010000 | all | exists | serial console (boot argument `serial=3`) | M1 |
| PL031 RTC | 0x20050000 | all | exists | — | — |
| PL061 + gpio-key power button | 0x20060000 (0x25) | all | exists | — | — |
| pvpanic-mmio | 0x20070000 | all; size 0x2 → 0x10000 in 27 | exists (2-byte window) | map the full 0x10000 window, reading zero beyond the device | M1 |
| bdif firmware block backdoor (aux, root, USB) | 0x30000000 | all | exists, read-only; USB function unimplemented | NVRAM writes (orchard); BDIF size note (aquarat); DEVID_USB virtual-DFU transport (steelbrain patch) for restores | M3 / M6 |
| cfg page | 0x400000 | all; grew in 13.0, 13.6, 27 | exists (12.x layout, 0x5a8 bytes) | random seed @0x5c8 (orchard); 27 fields 0x584/0x5c4/0x648; keep 0x6ec bit 0 clear | M1 / M3 |
| AES engine (aes,s8000) | 0x30220000 / 0x30230000 (0x32) | all | exists; buggy; dummy built-in keys | GnuTLS backend (present); w515y fixes 0004/0005 | M2 / M3 |
| PVG gfx / iosurface-mapper | 0x30200000 (0x31) / 0x30210000 (0x30) | all | host-only (wraps PVG.framework) | inert stubs that read 0 (driver backs out), then aquarat reims-vgpu-mmio | M1 → M4 |
| avp-rtc | 0x30240000 (0x33) | 14.0 | missing | RAZ first (falls back to PL031), then aquarat `avp-rtc.c` | M1 / M3 |
| avp-ctrr | 0x30260000 | 26.0 | missing | RAZ/WI only; never partially emulate (F16) | M1 |
| avp-sealed-registers | 0x30270000 | 27.0 | missing | RAZ/WI plus access logging; keep cfg 0x6ec bit 0 clear (F17) | M1 |
| PCIe GPEX: ECAM / MMIO window | 0x40000000 / 0x50000000 (0x40+) | all | exists; window 0x1fff0000 vs DT 0x2fff0000 | watch BAR placement; align to the DT if devices fail | M3 |
| vmapple-virtio-blk-pci (106b:1a00) root/aux | PCIe | all | exists | add the FileVault-off vendor capability (Mediouni v8 10/10) | M3 |
| virtio-net | PCIe | all | exists | `x-fix-overlapping-used` (aquarat; macOS 26 AppleVirtIO bug, 27 untested) | M3 |
| qemu-xhci + usb-kbd + usb-tablet | PCIe | all | exists | conditional-intr-mapping; MSIs through GICv2m for less input lag | M3 / M4 |
| virtio-input (type 18) | PCIe | ≥13 | exists | try virtio-keyboard/tablet (static analysis says they bind, gfx-22) | M5 |
| virtio-sound (25), balloon (5) | PCIe | 26.x | exist | aquarat patches (JACK_INFO/CHMAP_INFO OK; `macos-units`) | M5 |
| virtio-rng, console, vsock, 9p, fs | PCIe | — | exist | test; guest agent on vsock 505050 | M5 |
| Apple-private virtio: 0x1a03 VideoToolbox, 0x1a04 HID bridge, 0x1a07 power source, 0x1a08 FairPlay, 0x1a0c identity, 0x1a0d private vsock, 0x1a11 provisioning (new in 27) | PCIe | 13 to 27 | host-only (VZ) | leave absent; maybe stub 0x1a07 later; accept losing FairPlay/identity features (gfx-24) | — |
| Firmware ROM region (1 MiB) | 0x100000 | all | exists (`-bios`) | M1: our trampoline; M3: patched AVPBooter | M1 / M3 |
| cpu-debug-interface, port-virtual-1, pmgr (PSCI) | none | 14.0 | — | nothing to do | — |

---

## 6. Boot and disk plan (from Linux)

### 6.1 Mode 1: ggboot direct boot (M1, M2), no disk image needed
All inputs are already local:
- `recon/27.0.1/vma2/kernelcache.release.vma2.macho` (81.4 MB, decompressed);
- `DeviceTree.vma2macosap.raw` (61 KB template);
- static trust cache `…/Firmware/043-70867-640.dmg.aea.trustcache` (18,375 entries);
- ramdisk trust cache `043-70833-687.dmg.trustcache`;
- ramdisk `downloads/ramdisk-27.0.1/26A434__MacOS/043-70833-687.dmg` (IM4P `rdsk`; its APFS payload is passed as md0).

The pipeline is: build the ADT, `boot_args` and trust-cache blob, load them plus the kernel segments into guest RAM, then let the trampoline jump to the kernel.

**Security policy in this mode.** There is no iBoot, so no LocalPolicy is evaluated. We choose:
- `lp-sip0`/`lp-sip1` (SIP and authenticated root);
- `debug-enabled`;
- an unrestricted command line;
- extra cdhashes in trust-cache module 0.

Open item: whether AMFI honours `amfi_get_out_of_my_way` in this state (boot-23).

### 6.2 Mode 2: stock chain on the Tart 27.0 image (M3)
1. **Space.** Free at least 45 GB (§8.3).
2. **Disk.** Fetch the layers with an anonymous ghcr token and stream-decode them: Apple LZ4 `bv41` (`scripts/applelz4.py`), then ASIF, then a sparse raw image. dissect.hypervisor (AGPL) can do the conversion in a separate tool; or write our own from the CC0 spec. Verify each layer's uncompressed-digest annotation, then the GPT. Expect three containers: iBootSystemContainer 524 MB, Container 44.1 GB, RecoveryOSContainer 5.37 GB (boot-26).
3. **Aux.** Build `aux.img` from the NVRAM layer by dropping the 0x4000-byte VZ header. The 32 MiB store and trailer stay; whether the trailer matters is unknown (F30).
4. **ECID.** Use 0xb484f40bb3096b80 as the machine's `uuid` (it must match the tickets in aux and Preboot, boot-11).
5. **ROM.** Use AVPBooter from the image's own system volume (read with dissect.apfs), patched at the Image4 property-callback site (0x2370 in 27.0.1, F21).
6. **Boot.** AVPBooter → LLB (from aux, ticket bound to the ECID) → mBoot from Preboot/`<vg>`/boot/`<nsih>` → XNU (boot-27). The LocalPolicy was signed in-guest and is Full Security.
7. **Log in.** SSH as `admin`/`admin` through user-mode networking with `hostfwd`.
8. **Base image.** Keep the converted raw image read-only and work in qcow2 overlays.

### 6.3 Getting a policy that permits our patches
| Need | Route | Status |
|---|---|---|
| Patched or debug kernel, custom trust cache, any boot arguments | Mode 1 (direct boot): we set the DT knobs ourselves | preferred for all kernel-side debugging |
| SIP off or authenticated-root off on the real system | boot the VM's own recoveryOS, then `csrutil disable` / `bputil` (works in VZ, boot-24) | it is unknown how to request a recovery boot under QEMU (VZ uses `startUpFromMacOSRecovery`); investigate in M3/M5 |
| Same, without recoveryOS | mint a Reduced or Permissive LocalPolicy offline with the in-kernel test key and the NVRAM nonces (F29) | untested (U5) |
| Third-party kexts or custom kernel collections in the stock chain | the vz-kext-loader patch points (AVPBooter, LLB, iBoot, kernel `_validate_acm_context`, recovery iBoot and kernel) plus Permissive security (boot-24) | needed only if we ever ship guest kexts; avoid (D6) |
| Boot arguments on the stock chain | NVRAM append (the DT's normal environment has `allow-whitelist-disable=1`, effect unverified), or gdb injection at the XNU handoff | both used by prior art for 13 and 26 |

### 6.4 Our own install without an Apple-made image (M6), ranked
1. **Virtual-USB DFU restore.**
   - Implement the bdif `DEVID_USB` transport (decoded by steelbrain).
   - Bridge it to idevicerestore with personalization from Apple's signing server (TSS, gs.apple.com). The USB/IP route needs root for `vhci_hcd`, so use a user-space bridge or a helper Linux VM.
   - This path previously stalled after the iBSS upload (F31).
2. **Offline installer.**
   - Direct-boot the erase ramdisk (M2) with a blank disk attached.
   - Run Apple's newfs_apfs, asr, apfs_sealvolume and the cryptex tools inside it on images decrypted on Linux (AEA keys are public, F20).
   - Then personalize: LLB needs a personalized ticket or the patched AVPBooter. Everything after LLB can use the IPSW's global vma2 ticket plus an offline Reduced (smb0) policy (boot-37, corrected).
3. **A VM made on any Apple Silicon Mac**, if you have access to one (Q4). Copy `disk.img`, `aux.img` and `machineIdentifier` (ECID). This is the simplest option.
4. **Updates.** Try an in-guest `softwareupdate` from 27.0 to 27.0.1; it needs TSS from inside the guest, which is unknown under QEMU.

### 6.5 What Linux can and cannot do with APFS here
- **Read:** works with `dissect.apfs` (already in `tools/venv`) and our scripts; apfs-fuse is not installed.
- **Write:** linux-apfs-rw is experimental, needs root, and is therefore usable only inside a helper VM. The Data and Preboot volumes are unencrypted (F27), so offline edits are possible in principle. Never touch the sealed System volume.
- **Create a bootable layout:** not possible on Linux (F28). It has to come from Apple's tools running in the guest (M6).

---

## 7. Graphics and input plan

| Stage | What | Notes and sources |
|---|---|---|
| G0 (M1 to M3) | Inert PVG stubs that read 0 | AppleParavirtGPU logs "backing out" and does not start (F32). Expect WindowServer to crash-loop (F33); if it wastes CPU under TCG, disable it in the overlay. |
| Consoles | PL011 serial (`serial=3`); SSH over virtio-net; later AppleQEMUGuestAgent over vsock 505050 (U6) | 27's PL011 driver and DT `serial-device` are confirmed (gfx-15) |
| Optional | Kernel text console through `boot_args.Video` with QEMU ramfb | Cosmetic only; mBoot and the DT give no PVG boot framebuffer (gfx-17) |
| G1 (M4) | aquarat's `reims-vgpu` fork (LGPL-3.0, pinned `294f1489b046`) + `metal2vulkan` (pinned `43c46ac8a24a`), integrated through aquarat's `reims-vgpu-mmio` shim (`qemu-reims-vgpu` `2cd151d3b483`) | Upstream Reims lacks the macOS 26+ mapper layout (F34) |
| G1 host side | Guest RAM on `memory-backend-memfd,share=on`; Linux page views (`MAP_SHARED`/`MAP_FIXED`); xreg and KVA reads under TCG | An arm64 guest on an x86 host with 4K host pages is an untested Reims combination (gfx open question) |
| G1 protocol | The guest proposes 252, Reims answers 62, which is a valid rung (F32), so every feature above rung 62 stays off | Check whether 27's user-space driver (71.x) accepts serializer version 8 |
| G1 deltas for 27 | IOSurfaceDeviceDescriptor2 +0x60 client offset (non-page-aligned surfaces); new commands (`patchNoppedCommands`, `addCommandMulti`, `deleteHostResource`, `setupCDHash`); brightness/EDR caps; 71.x serializer records; AIR bitcode version in 27's system metallibs (metal2vulkan uses llvm-dis 23 and spirv-val, both installed) | gfx-10, gfx (missed) |
| G1 reverse-engineering sources | Guest bundle `AppleParavirtGPUMetalIOGPUFamily.bundle` (system volume). Host-side `ParavirtualizedGraphics.framework` lives in the dyld cache inside Cryptex1 SystemOS (AEA, key public) plus the GPUTask XPC service. Guest-side captures. | Reims' "oracle" needs macOS to run Apple's serializer, which this host cannot do (gfx-35, corrected) |
| G1 resources | RX 7600 XT via RADV; guest RAM ≤ 8 GiB (orchard saw ~10.5 GiB host-import limits on its class of host); set `REIMS_VGPU_GUEST_IMPORT=off` if import fails | VRAM is 12.9 of 16 GiB used by llama.cpp now, so stop it during GUI work (Q3) |
| Display | Reims host window (Wayland or X11, with keyboard-shortcut grab) or QEMU VNC/GTK when the Reims window is off | rebuild gg-qemu with GTK/VNC/OpenGL; the current local build has none |
| Input | `qemu-xhci` + `usb-kbd` + `usb-tablet` (absolute pointer), proven on 13 and 26; MSIs through GICv2m fix the lag of the legacy-IRQ era | macOS has only XHCI host drivers (gfx-21) |
| Later (M5) | virtio-sound; clipboard through tart-guest-agent's vdagent plus QEMU `qemu-vdagent`; VideoToolbox paravirtualization (0x1a03) only if ever needed | gfx-26, gfx-28 |
| Fallback | If PVG on 27 cannot be made to work, stay headless | Not recommended: a software WindowServer (SkyLight/QuartzCore patching, shown only for iOS 14) or a PVG host written from scratch (gfx-19, gfx-33) |

---

## 8. Tooling, downloads and budgets

### 8.1 Downloads (sizes from HEAD requests and registry manifests [S])

| Item | URL | Bytes | Status | Needed for |
|---|---|---|---|---|
| macOS 27.0.1 IPSW | https://updates.cdn-apple.com/2026FallFCS/59241290-5d51-4ca8-9df4-31624b9a4eac/UniversalMac_27.0.1_26A434_Restore.ipsw | 26,637,307,067 | wget exited ~23:43; completeness unverified (U9) | M6 only; M1 to M5 use range extraction |
| macOS 27.0 IPSW | https://updates.cdn-apple.com/2026FallFCS/afcfc88e-bbe6-44bf-a5da-07c56eebc06c/UniversalMac_27.0_26A428_Restore.ipsw | 26,626,436,228 | range-extract only | 27.0 artifacts matching the Tart image, if needed |
| macOS 26.6.2 IPSW | https://updates.cdn-apple.com/2026SummerFCS/fullrestores/140-75212/A2A24B94-1FC1-45A3-93F7-C51B02AF1F4D/UniversalMac_26.6.2_25G83_Restore.ipsw | 19,772,231,540 | range-extract only | comparisons, R1 |
| Tart `macos-golden-gate-vanilla:27.0` | `https://ghcr.io/v2/cirruslabs/macos-golden-gate-vanilla/manifests/27.0` (blobs under `/blobs/<digest>`, anonymous token from `ghcr.io/token`) | 30,867,739,355 compressed (67 layers) + aux 33,579,164 + config 510; expands to 35,735,707,648 ASIF holding a 50,000,000,000 B disk | first layer and aux local (`downloads/tart-golden-gate-vanilla-27.0/`, 1.0 GB) | **M3** |
| Tart `macos-golden-gate-base:latest` (alternative) | `ghcr.io/v2/cirruslabs/macos-golden-gate-base/manifests/latest` | 34,098,689,100 compressed (77 layers) → 41,046,507,520 ASIF | not needed | — |
| Tart `macos-ventura-base:latest` (orchard's known-good) | `ghcr.io/v2/cirruslabs/macos-ventura-base/manifests/latest` | 22,583,748,803 compressed (94 layers) → 50,000,000,000 raw | optional baseline | M0 (only if space) |
| KDK 27.0.1 | https://github.com/dortania/KdkSupportPkg/releases/download/26A434/Kernel_Debug_Kit_27.0.1_build_26A434.dmg | 1,245,349,997 | optional; or range-extract the vmapple kernel and dSYM | M1 to M3 symbols |
| 26.7.1 InstallAssistant | https://swcdn.apple.com/content/downloads/51/17/142-28081-A_616Q67X32N/5atqz1miz61kmrim7fwkn7yi62xuzzcc0s/InstallAssistant.pkg | 18,380,974,544 | downloaded to `downloads/macos-26.7.1-25G241-installer/` (wget exited) | R1 only, so a deletion candidate |
| 27.0.1 InstallAssistant | https://swcdn.apple.com/content/downloads/04/52/142-27367-A_W3GHHHB05E/4kfhtjlg2i1qcv0obh62hlfcambu6kv15n/InstallAssistant.pkg | 18,386,308,006 | not needed | — |

**Source repositories.** Sizes are GitHub's packed `size`; fetch shallow at the pinned commit.

| Repo | Size (KB) | Pinned commit | Licence |
|---|---|---|---|
| yaelliethy/orchard | 48,670 | `c73a2bda1fd4` | GPL-2.0-or-later QEMU changes; reims LGPL-3.0 |
| aquarat/qemu-reims-vgpu | 675,112 | `2cd151d3b483` | GPL-2.0 |
| aquarat/reims-vgpu | 22,505 | `294f1489b046` | LGPL-3.0 |
| steelbrain/metal2vulkan | 46,695 | `43c46ac8a24a` | LGPL-3.0 |
| steelbrain/experiment-macOS-arm64-on-linux-x86 | 390,933 | `509a4ce52bc7` | reference only |
| aquarat/apple-macos-arm64-vm-guest | 259 | — | GPL-2.0 |
| DenrianWeiss/w515y-vmapple-scripts | 76 | — | GPL-2.0 |
| AsahiLinux/m1n1 | 4,119 | — | GPL-2.0 |
| libimobiledevice/idevicerestore | 1,329 | — | LGPL-3.0; M6 |
| fox-it/dissect.hypervisor and dissect.apfs | — | — | AGPL-3.0; Python tools only |
| huven/asif-format | — | — | CC0 |

**Already local:**
- AVPBooter 27.0.1 (`recon/27.0.1/avpbooter/`);
- all M1/M2 inputs (§6.1);
- QEMU 11.1.2 source, GPG-verified (`tools/qemu-src/`).

### 8.2 Tools on this host [S]
**Present:**
- QEMU 11.1.2 TCG build: GnuTLS cipher backend, slirp; no GTK/SDL/VNC/OpenGL and no `qemu-img` yet.
- gdb 18.1 (aarch64 OK), clang/lld/LLVM 23 (llvm-dis, llvm-objdump), spirv-val/spirv-dis, glslangValidator, vulkaninfo.
- Rust 1.99 stable through `RUSTUP_TOOLCHAIN=stable` (rustup has no default toolchain set).
- uv venv `tools/venv` (capstone, dissect.*), blacktop ipsw 3.1.736, rizin, Ghidra 12.1 (its MCP server is down), bwrap.

**Libraries available for QEMU:** gnutls 3.8.13, nettle 4.0, libgcrypt, glib, pixman, slirp, libusb, vulkan, epoxy, gbm, SDL2, GTK3, zstd, lz4, usbredir.

**Missing:** apfs-fuse, system-wide `qemu-img`, `meson` on PATH (QEMU bundles it), virglrenderer (not needed).

**No sudo,** so no kernel modules on the host (linux-apfs-rw, vhci-hcd). Use a helper VM under KVM instead.

### 8.3 Disk budget (`/home`, btrfs with zstd:1 compression)

| | GB |
|---|---|
| Free now (fluctuating, F45) | 14 to 26 |
| Reclaimable, your decision (Q2): 26.7.1 InstallAssistant | 18.4 |
| Reclaimable: 27.0.1 IPSW (only for M6) | 26.6 |
| Reclaimable: research downloads (`downloads/x86code` 1.7, `factcheck-x86code` 0.76), core dump in `tools/` (0.21), Tart layer00 blob after conversion (0.43) | ~3.1 |
| **Need: M0 to M2** (gg-qemu source and build ~1.5 to 2.5, KDK optional 1.25, logs) | ~3 to 5 |
| **Need: M3** (sparse raw 27.0 disk, at most 35.7 allocated, plus overlay 2 to 8) | ~38 to 44 |
| **Need: M4** (Rust toolchain targets for Reims and metal2vulkan, shader caches; estimate) | ~5 to 10 |
| **Need: M5** (snapshots and overlays) | ~5 |
| **Need: M6** (IPSW 26.6 + new 50 GB disk + scratch) | ~60 to 80 |

Conclusion: M0 to M2 fit today. M3 and M4 need about 50 to 60 GB, so delete the 26.7.1 installer and the 27.0.1 IPSW (about 45 GB) or add storage. M6 needs extra storage either way.

### 8.4 RAM, VRAM and CPU budget

| Mode | Guest RAM | QEMU/TCG overhead | GPU | Host total | Fits today? |
|---|---|---|---|---|---|
| M1 to M2 direct boot, 1 to 4 vCPUs | 2 to 4 GiB | ~1 to 1.5 GiB (TB cache 1 GiB default, xlat-14) | — | 3 to 5.5 GiB | yes |
| M3 headless, 4 to 6 vCPUs | 6 GiB (VZ minimum 4 GiB, vm-03) | ~1.5 to 2.5 GiB (`tb-size` up to 2 GiB) | — | 7.5 to 8.5 GiB | yes, but tight (12.9 GiB available, heavy swap) |
| M4 GUI, 6 to 8 vCPUs | 8 GiB | ~2.5 GiB | 1 to 2 GiB host + 1 to 3 GiB VRAM | 11.5 to 12.5 GiB | only with the Windows VM paused and llama.cpp stopped (Q3) |

- **CPU.** There are 6 cores and 12 threads. MTTCG threads compete with the Windows VM (2 vCPUs) and llama.cpp. Start at 4 to 6 vCPUs and sweep up to 12 (F38).
- **Hugepages.** THP is already `always`.

---

## 9. Risk register

L = likelihood and I = impact, each rated H, M or L.

| # | Risk | L | I | Mitigation |
|---|---|---|---|---|
| R1 | Host disk runs full: 14 to 26 GB free and fluctuating; M3 and M4 need about 50 to 60 GB | H | H | Q2 decisions before M3; stream conversions straight to a sparse raw image; qcow2 overlays; remote range extraction instead of whole downloads; watch `df` in scripts and abort below 5 GB |
| R2 | macOS 27-only boot requirements stall the boot: sealed registers, identity/provisioning virtio (0x1a0c, 0x1a11), HostKeys, new cfg fields | M | H | Direct-boot debugging with full DT control; RAZ windows with logging; KDK symbols; gdbstub; compare with 26.x guests and aquarat's 26.4 notes; never emulate half a device (F16) |
| R3 | PAC semantics wrong: JOP panics, launchd SIGBUS, SMP livelock | M | H | Follow orchard's contract (F9); keep impdef PAC; SMP soak tests in M2 |
| R4 | The Tart 27.0 image won't run under QEMU (descriptor v2, VZ-key AES/keybag, HostKeys) (U4) | M | H | Test in week 1 of M3; fall back to a VM made on a real Mac (Q4) or to M6 |
| R5 | AVPBooter 27 behaves differently (patch site, new probes) | M | M | Static site 0x2370 (F21); confirm in the M0 smoke test; Mode 1 does not need AVPBooter at all |
| R6 | TCG is too slow for comfortable GUI use | H | M | Headless first; M5 performance work (PAC fast path, 16K TLB, WFK/KICK, vCPU sweep); set expectations (§1) |
| R7 | PVG protocol deltas in 27 (71.x serializer, mapper, IOSurface offset, AIR version) block the GUI | H | H (for M4) | Start from aquarat's fork; rung 62; reverse the 27 guest and host binaries statically; guest captures; stay headless if it fails |
| R8 | 16K "large page" full TLB flushes plus synchronized cross-vCPU TLBI kill SMP scaling (F38) | M | M | QEMU patch (variable-size TLB entries or 14-bit target pages); measure per vCPU count |
| R9 | Watchdogs and timeouts fire under slow emulation (launchd, WindowServer, userspace watchdogd) | M | M | Correct 24 MHz timebase (F10); relax watchdogs by boot argument or service overrides where honoured; profile with `-d unimp` and gdb |
| R10 | Upstream churn or abandonment: Reims upstream idle; orchard has one author; Mediouni's series unmerged; 27.x updates | M | M | Pin commits; vendor patches; own the patch queue; standardize on 27.0 (Q8) |
| R11 | Supply chain: building third-party QEMU forks and Rust crates runs their code on the host | M | H | Review diffs; pin commits; `cargo --locked`; run QEMU unprivileged (optionally under `bwrap`); never run downloaded binaries |
| R12 | Host contention: RAM 12.9 GiB available with 19 GiB swapped; VRAM 12.9 of 16 GiB used; Windows VM; games | H | M | Schedule emulator sessions; pause the other workloads (Q3); size guest RAM at 4 to 8 GiB |
| R13 | Apple services fail: Apple Account/iCloud, App Store, FairPlay/DRM, Rosetta in the guest | H | L to M | Accept and document; out of scope (F30, gfx-24) |
| R14 | Dependence on Apple's signing server (TSS) for fresh installs or updates; Apple can stop signing | M | M | Use the pre-installed image; keep personalized blobs; research offline policies (U5) |
| R15 | **Legal.** (a) Apple's licence allows macOS virtualization only on Apple-branded hardware (F46), so this use is outside it. (b) Redistributing Apple binaries (IPSW contents, AVPBooter, kernelcaches, KDK, Tart images, AEA keys, tickets) is not allowed. (c) Patching Image4 checks or decrypting AEA may raise anti-circumvention questions, depending on jurisdiction. (d) Code licences: QEMU is GPL-2.0 as a whole (F47), Reims is LGPL-3.0, and Inferno and dissect are AGPL-3.0. | certain for (a) | H | Private interoperability research only (your call, Q5). Never redistribute Apple material: ship only our code and patches plus scripts that users run on their own downloads. Keep keys and blobs out of any repository. Keep AGPL code out of the QEMU tree. Check LGPL-3.0/GPLv2 compatibility before distributing binaries (private builds are unaffected). Get legal advice before publishing patches that disable Apple security checks. None of this is legal advice. |
| R16 | Self-reported prior art is wrong (U1, U2) | M | L | Treat it as hints only |
| R17 | Guest image corruption from crashes or experimental APFS writes | M | M | Read-only base plus qcow2 overlays; checksums; edit APFS only inside a helper VM |
| R18 | Hidden IMPDEF, AMX or SME paths raise UNDEF | L | M | RAZ/WI stubs; leave `guest_amx_version` unset; log with `-d unimp` |
| R19 | The PCIe MMIO window mismatch breaks BAR allocation as devices are added | L | M | Align the window to the DT (0x2fff0000) if it happens |

---

## 10. Open questions only you can decide

- **Q1. Goal.** Is a headless macOS 27 (SSH and command line, e.g. for builds or tests) already useful to you, or is a graphical desktop the real goal? Which apps or workloads matter? Is "works, but each core is roughly 10 to 17× slower than native" acceptable?
- **Q2. Disk.** May I plan on deleting these, after verification where relevant?
  - the 26.7.1 InstallAssistant (18.4 GB; only useful for the Track C spike);
  - the 27.0.1 IPSW (26.6 GB; only needed for M6; everything else can be range-extracted).

  Or will you add about 60 GB or more of storage? M3 cannot start otherwise.
- **Q3. Shared resources.** During emulator sessions (especially M4), may the Windows VM (8 GiB) and the llama.cpp servers (12.9 GiB VRAM) be paused? What is the maximum RAM and how many host threads may the guest use?
- **Q4. Base image.** Is it acceptable to start from Cirrus Labs' pre-installed macOS 27.0 image (Apple software installed by a third party on Apple hardware; admin/admin)? Do you have access, even briefly, to an Apple Silicon Mac? It would let us create our own VM bundle and capture ground truth: an `ioreg -p IODeviceTree` dump, serial logs, and PVG traces.
- **Q5. Legal posture and publishing.** Do you confirm this is private interoperability research, accepting that Apple's licence restricts macOS to Apple-branded hardware? Should our QEMU patches be published (GPL-2.0-or-later) and shared with the orchard, aquarat and Mediouni efforts, or kept private? Are there jurisdiction constraints I should know about?
- **Q6. Network.** May tools contact Apple's signing server (gs.apple.com) for personalization tests in M6, and may the guest reach Apple's update servers? An earlier TSS probe was blocked by the permission system.
- **Q7. Track C.** Park it, or fund the 2 to 4 person-month R1 spike (x86_64 Darwin 27 kernel to a CLI shell under KVM)? It is likely to fail and is not a route to a GUI.
- **Q8. Version pin.** Is standardizing on macOS 27.0 (26A428, the Tart image) for M3 to M5 acceptable, with 27.0.1 and 27.x updates treated as M6 work?
- **Q9. Time budget and checkpoints.** How many hours per week? Where do you want approval gates, for example before each large download, before network calls to Apple, and before running any third-party fork?

---

## Appendix A: evidence produced in this session [S]

All times are 2026-10-10 CEST. About 1.8 MB was downloaded, plus metadata requests.

**Host state:**
- `df -B1 /home`: 14.3 to 17.2 GB available (23:40 to 23:47), one dip to 3.5 GB, 26.0 GB at 00:01; btrfs, compress=zstd:1.
- `free -b`: 12.9 GiB available, 19.2 GiB swap used.
- `/proc/3460/cmdline`: the Windows VM runs with `-m 8G -smp 2`.
- `/sys/class/drm/card1/device/mem_info_vram_*`: 12.9 of 16 GiB VRAM used.
- `/proc/*/io`: EA Desktop (Wine) was writing about 39 MB/s; both wget processes had exited by about 23:43.

**Toolchain:** `config-host.h` in the local QEMU build defines `CONFIG_GNUTLS_CRYPTO`; there is no vmapple machine and no UI backends; gdb handles aarch64; Rust 1.99 works through `RUSTUP_TOOLCHAIN=stable`.

**Sizes:** from HEAD requests on all Apple URLs in §8.1 and the GitHub release API for the KDK; anonymous ghcr manifests for the Ventura, Golden Gate base and Tahoe base images.

**Image template:** `cirruslabs/macos-image-templates/templates/vanilla-golden-gate.pkr.hcl` has `from_ipsw` 27.0 26A428, `ssh_username`/`ssh_password` admin, and a NOPASSWD sudoers entry.

**AVPBooter:**
- Extracted with `tools/venv/bin/python -I scripts/apfs_remote.py <27.0.1 IPSW URL> 043-70867-640.dmg.aea <FS key> recon/27.0.1/avpbooter /System/Library/Frameworks/Virtualization.framework/Versions/A/Resources/AVPBooter.vmapple2.bin`.
- Result: 334,960 B, sha256 `6d010540a047ec71a54cb96565171f8f2e0e191dd5dbef2403f3863409afa845`.
- Static check: `python3 -I scripts/avpbooter_find_cepo.py …`, output in `recon/27.0.1/avpbooter/patch-site-check.txt`. 0x2314 holds `e2230091 66010194` (orchard's bytes are not there). The function at 0x2370 (566 instructions) builds AMNM/BNCH/BORD/CEPO/CHIP/CPRO/CSEC/SDOM/ECID/DGST/E*/D* Image4 tags and starts with `7f2303d5 ff4303d1`.
- Note: orchard's script comment says `sub sp,sp,#0x10`, but its expected bytes encode `#0xd0`.

**Prior-art pins:** from `gh api repos/<repo>` and `repos/<repo>/commits/<default>`, listed in §8.1.

## Appendix B: glossary
- **TCG / MTTCG.** QEMU's dynamic binary translator, which can run one host thread per guest vCPU.
- **HVF / KVM.** Hardware virtualization on macOS and Linux. Neither can run arm64 guests on an x86 CPU.
- **VirtualMac2,1 / vma2 / vmapple.** Apple's virtual-Mac board, the one Virtualization.framework uses.
- **AVPBooter.** The VM boot ROM. Boot order: LLB (stage 1, stored in the aux file), then iBoot/mBoot (stage 2, on the Preboot volume), then XNU.
- **aux file.** VZ's auxiliary storage: LLB slots, NVRAM, tickets and a trailer.
- **ECID.** The unique chip ID that tickets are bound to.
- **TSS.** Apple's signing server, which personalizes tickets. **LocalPolicy** is the per-volume boot security policy (SIP, Reduced/Permissive).
- **PAC.** ARM pointer authentication; arm64e code depends on it.
- **PVG.** Apple's ParavirtualizedGraphics GPU interface. **Reims** is an open re-implementation of the host side that renders through Vulkan.
- **ASIF.** Apple Sparse Image Format, the disk format in the Tart 27 images.
- **RAZ/WI.** Read-as-zero, write-ignored.
- **DT / ADT.** Apple device tree.
