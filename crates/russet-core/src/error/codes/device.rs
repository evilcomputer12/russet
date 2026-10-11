// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `device` error codes (3000–3099, 3200–3299). Owner: W4 (Wave 1), W10 (Wave 2: the 3200 device-services range).
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 6 (device state), except `device.notFound` (3). The old device.md kinds map as in ARCHITECTURE §13.3.

use crate::error::ErrorCode;

/// 3000 `device.notFound`: No device matches the selector (device.md `device.not_found`).
pub const NOT_FOUND: ErrorCode = ErrorCode::new(3000, "device.notFound", 3);
/// 3001 `device.notPaired`: The device is not paired (`pairing.required`).
pub const NOT_PAIRED: ErrorCode = ErrorCode::new(3001, "device.notPaired", 6);
/// 3002 `device.locked`: The device is locked (`device.locked`).
pub const LOCKED: ErrorCode = ErrorCode::new(3002, "device.locked", 6);
/// 3003 `device.untrusted`: The user denied trust (`pairing.denied`).
pub const UNTRUSTED: ErrorCode = ErrorCode::new(3003, "device.untrusted", 6);
/// 3004 `device.developerModeDisabled`: Developer Mode is off (`devmode.off`).
pub const DEVELOPER_MODE_DISABLED: ErrorCode = ErrorCode::new(3004, "device.developerModeDisabled", 6);
/// 3005 `device.ddiNotMounted`: No Developer Disk Image is mounted (`ddi.missing`).
pub const DDI_NOT_MOUNTED: ErrorCode = ErrorCode::new(3005, "device.ddiNotMounted", 6);
/// 3006 `device.tunnelFailed`: The CoreDevice tunnel could not be opened (`tunnel.failed`).
pub const TUNNEL_FAILED: ErrorCode = ErrorCode::new(3006, "device.tunnelFailed", 6);
/// 3007 `device.disconnected`: The device went away (`device.disconnected`).
pub const DISCONNECTED: ErrorCode = ErrorCode::new(3007, "device.disconnected", 6);
/// 3008 `device.usbmuxUnavailable`: usbmuxd (or AMDS) is unavailable (`mux.unavailable`).
pub const USBMUX_UNAVAILABLE: ErrorCode = ErrorCode::new(3008, "device.usbmuxUnavailable", 6);
/// 3009 `device.trustPending`: The trust dialog is pending on the device (`pairing.pending`).
pub const TRUST_PENDING: ErrorCode = ErrorCode::new(3009, "device.trustPending", 6);
/// 3010 `device.ddiPersonalizationFailed`: DDI personalization (TSS) failed (`ddi.personalization_failed`).
pub const DDI_PERSONALIZATION_FAILED: ErrorCode = ErrorCode::new(3010, "device.ddiPersonalizationFailed", 6);
/// 3011 `device.tunnelStalled`: The tunnel stopped making progress (`tunnel.stalled`).
pub const TUNNEL_STALLED: ErrorCode = ErrorCode::new(3011, "device.tunnelStalled", 6);
/// 3012 `device.serviceUnavailable`: The device service is unavailable (`service.unavailable`).
pub const SERVICE_UNAVAILABLE: ErrorCode = ErrorCode::new(3012, "device.serviceUnavailable", 6);
/// 3013 `device.serviceNotImplemented`: The device service is not implemented (`service.not_implemented`).
pub const SERVICE_NOT_IMPLEMENTED: ErrorCode = ErrorCode::new(3013, "device.serviceNotImplemented", 6);
/// 3014 `device.timeout`: A device request timed out (`timeout`).
pub const TIMEOUT: ErrorCode = ErrorCode::new(3014, "device.timeout", 6);
/// 3015 `device.unsupported`: Unsupported on this device or iOS version (`unsupported`).
pub const UNSUPPORTED: ErrorCode = ErrorCode::new(3015, "device.unsupported", 6);
/// 3016 `device.io`: Device I/O failed (`io`).
pub const IO: ErrorCode = ErrorCode::new(3016, "device.io", 6);
/// 3017 `device.protocol`: A device protocol error (`protocol`).
pub const PROTOCOL: ErrorCode = ErrorCode::new(3017, "device.protocol", 6);
/// 3018 `device.reserved`: Another principal holds the device reservation. Was RUS-DEV-0423.
pub const RESERVED: ErrorCode = ErrorCode::new(3018, "device.reserved", 6);
/// 3019 `device.liveBackendRefused`: Development guard: the `live` backend is refused without `RUSSET_LANE=A`, or another daemon holds the live-device lock (`details.holder {pid, home}`) (ARCHITECTURE §3.3 item 7).
pub const LIVE_BACKEND_REFUSED: ErrorCode = ErrorCode::new(3019, "device.liveBackendRefused", 6);
/// 3020 `device.ddiSourceMissing`: No imported DDI and no configured directory; hint: `russet sdk import --ddi-only <Xcode_27.xip | XcodeSystemResources.pkg | a copy of a Mac's /Library/Developer/DeveloperDiskImages/iOS_DDI>`.
pub const DDI_SOURCE_MISSING: ErrorCode = ErrorCode::new(3020, "device.ddiSourceMissing", 6);
/// 3021 `device.ddiIncompatible`: The DDI does not fit this iOS version or board; `ensure_ddi` tries the next imported build.
pub const DDI_INCOMPATIBLE: ErrorCode = ErrorCode::new(3021, "device.ddiIncompatible", 6);
/// 3022 `device.ddiMirrorConsentRequired`: `pmd3-cache` or `pmd3-auto-mount` without the `allow-mirror` consent.
pub const DDI_MIRROR_CONSENT_REQUIRED: ErrorCode = ErrorCode::new(3022, "device.ddiMirrorConsentRequired", 6);
/// 3200 `device.fallbackUnavailable`: The pymobiledevice3 fallback is unavailable (`fallback.unavailable`).
pub const FALLBACK_UNAVAILABLE: ErrorCode = ErrorCode::new(3200, "device.fallbackUnavailable", 6);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[
    NOT_FOUND,
    NOT_PAIRED,
    LOCKED,
    UNTRUSTED,
    DEVELOPER_MODE_DISABLED,
    DDI_NOT_MOUNTED,
    TUNNEL_FAILED,
    DISCONNECTED,
    USBMUX_UNAVAILABLE,
    TRUST_PENDING,
    DDI_PERSONALIZATION_FAILED,
    TUNNEL_STALLED,
    SERVICE_UNAVAILABLE,
    SERVICE_NOT_IMPLEMENTED,
    TIMEOUT,
    UNSUPPORTED,
    IO,
    PROTOCOL,
    RESERVED,
    LIVE_BACKEND_REFUSED,
    DDI_SOURCE_MISSING,
    DDI_INCOMPATIBLE,
    DDI_MIRROR_CONSENT_REQUIRED,
    FALLBACK_UNAVAILABLE,
];
