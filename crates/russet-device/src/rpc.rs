// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The device methods of ARCHITECTURE §8.2 over a [`DeviceManager`]: params DTO in, result DTO out. russetd's
//! handlers (`handlers/{device,app,process,forward,logs}.rs`) are thin wrappers that add what needs the
//! connection (leases, notifications, the reservation principal); everything else is here, testable without a
//! daemon.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use russet_core::Result;
use russet_core::error::codes;
use russet_core::ids::LeaseId;
use russet_core::progress::ProgressSink;
use russet_protocol::{app as pa, common, device as pd, forward as pf, logs as pl, process as pp};

use crate::debug::{BindSpec, ForwardHandle};
use crate::error::{device_error, invalid_argument};
use crate::logs::{LogFilter, LogSourceKind, LogStream, TailQuery};
use crate::manager::{DeviceEvent, DeviceHandle, DeviceManager};
use crate::model::{DeviceSelector, Udid};
use crate::pairing::{PairOutcome, PrepareOptions};
use crate::process::{LaunchRequest, LaunchVia, LaunchedProcess};
use crate::wire;

/// How long `device.list` waits for each device's identity (the < 300 ms acceptance is for a warm daemon).
pub const LIST_IDENTITY_BUDGET: Duration = Duration::from_millis(1500);

/// Who calls: the connection id and its shared reservation lease (ARCHITECTURE §6.3) [D 68].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Caller {
    pub conn: String,
    pub shared_lease: Option<LeaseId>,
}

/// Resolves `udid?` (a UDID or alias).
pub fn handle(m: &DeviceManager, udid: Option<&str>) -> Result<DeviceHandle> {
    m.get(&DeviceSelector::parse(udid))
}

/// Resolves `udid?` and checks the reservation (`device.reserved`, 3018) for device-class methods.
pub fn handle_for(m: &DeviceManager, udid: Option<&str>, caller: &Caller) -> Result<DeviceHandle> {
    let h = handle(m, udid)?;
    m.check_access(h.udid(), &caller.conn, caller.shared_lease.as_ref())?;
    Ok(h)
}

pub async fn device_list(m: &DeviceManager) -> Result<pd::DeviceListResult> {
    let devices = m.list(LIST_IDENTITY_BUDGET).await?;
    Ok(pd::DeviceListResult { devices: devices.iter().map(wire::summary).collect() })
}

pub async fn device_info(m: &DeviceManager, p: pd::DeviceInfoParams) -> Result<pd::DeviceInfoResult> {
    let h = handle(m, p.udid.as_deref())?;
    Ok(wire::info(&h.info().await?))
}

pub async fn device_readiness(m: &DeviceManager, p: pd::DeviceReadinessParams) -> Result<common::Readiness> {
    let h = handle(m, p.udid.as_deref())?;
    Ok(wire::readiness(&h.readiness(p.refresh.unwrap_or(false)).await?))
}

pub async fn device_prepare(m: &DeviceManager, c: &Caller, p: pd::DevicePrepareParams) -> Result<common::Readiness> {
    let h = handle_for(m, p.udid.as_deref(), c)?;
    let opts = PrepareOptions {
        ddi_source: p.ddi_source.as_ref().map(wire::ddi_source),
        reveal_developer_mode: p.reveal_developer_mode.unwrap_or(false),
    };
    Ok(wire::readiness(&h.prepare(opts).await?))
}

pub async fn device_pair(m: &DeviceManager, c: &Caller, p: pd::DevicePairParams) -> Result<pd::DevicePairResult> {
    let h = handle_for(m, p.udid.as_deref(), c)?;
    let status = match h.pair(p.host_name.as_deref()).await? {
        PairOutcome::Paired => pd::PairStatus::Paired,
        PairOutcome::Pending => pd::PairStatus::Pending,
        PairOutcome::Denied => pd::PairStatus::Denied,
    };
    Ok(pd::DevicePairResult { status })
}

pub async fn device_ddi_status(m: &DeviceManager, p: pd::DeviceDdiStatusParams) -> Result<common::DdiStatus> {
    let h = handle(m, p.udid.as_deref())?;
    Ok(wire::ddi_status(&h.ddi_status().await?))
}

pub async fn device_ddi_mount(
    m: &DeviceManager,
    c: &Caller,
    p: pd::DeviceDdiMountParams,
    progress: &dyn ProgressSink,
) -> Result<common::DdiStatus> {
    let h = handle_for(m, p.udid.as_deref(), c)?;
    let status = match &p.source {
        Some(src) => h.ddi_mount(&wire::ddi_source(src), p.force.unwrap_or(false), progress).await?,
        None if p.force.unwrap_or(false) => {
            let dir = h.cfg().ddi.dir.clone().ok_or_else(|| {
                device_error(
                    codes::device::DDI_SOURCE_MISSING,
                    "`force` needs a source, and `[device.ddi] dir` is not set",
                )
            })?;
            h.ddi_mount(&crate::ddi::DdiSource::Dir(dir), true, progress).await?
        }
        None => h.ensure_ddi().await?,
    };
    Ok(wire::ddi_status(&status))
}

pub fn device_reserve(m: &DeviceManager, c: &Caller, p: pd::DeviceReserveParams) -> Result<pd::DeviceReserveResult> {
    let h = handle(m, p.udid.as_deref())?;
    let purpose = if p.purpose.trim().is_empty() { "reserved".to_owned() } else { p.purpose };
    let r = m.reserve(&h, &c.conn, &purpose)?;
    Ok(pd::DeviceReserveResult { lease_id: r.lease })
}

pub fn device_alias_set(m: &DeviceManager, p: pd::DeviceAliasSetParams) -> Result<pd::DeviceAliasSetResult> {
    m.set_alias(&p.alias, &Udid(p.udid))?;
    Ok(pd::DeviceAliasSetResult {})
}

/// A registry event as `device.changed` content.
pub fn device_change(ev: &DeviceEvent) -> (pd::DeviceChange, russet_protocol::common::DeviceSummary) {
    match ev {
        DeviceEvent::Added(s) => (pd::DeviceChange::Added, wire::summary(s)),
        DeviceEvent::Changed(s) => (pd::DeviceChange::Updated, wire::summary(s)),
        DeviceEvent::Removed(u) => (
            pd::DeviceChange::Removed,
            wire::summary(&crate::model::DeviceSummary::attached(u.clone(), crate::model::Transport::Usb)),
        ),
    }
}

pub async fn app_list(m: &DeviceManager, p: pa::AppListParams) -> Result<pa::AppListResult> {
    let h = handle(m, p.udid.as_deref())?;
    let filter = crate::apps::AppFilter { all: p.kind == Some(pa::AppKind::All), bundle_id_prefix: p.bundle_id_prefix };
    Ok(pa::AppListResult { apps: h.apps(filter).await?.iter().map(wire::app).collect() })
}

pub async fn app_install(
    m: &DeviceManager,
    c: &Caller,
    p: pa::AppInstallParams,
    progress: &dyn ProgressSink,
) -> Result<pa::AppInstallResult> {
    let h = handle_for(m, p.udid.as_deref(), c)?;
    let path = PathBuf::from(&p.path);
    if !path.is_absolute() {
        return Err(invalid_argument(format!("{:?} is not an absolute path (ARCHITECTURE §7.3)", p.path)));
    }
    let installed = h.install(&path, crate::apps::InstallOptions::default(), progress).await?;
    Ok(pa::AppInstallResult {
        bundle_id: installed.bundle_id,
        installed_path: installed.installed_path.unwrap_or_default(),
        duration_ms: installed.duration.as_millis() as u64,
    })
}

pub async fn app_uninstall(m: &DeviceManager, c: &Caller, p: pa::AppUninstallParams) -> Result<pa::AppUninstallResult> {
    let h = handle_for(m, p.udid.as_deref(), c)?;
    h.uninstall(&p.bundle_id).await?;
    Ok(pa::AppUninstallResult {})
}

/// `app.launch`: the RPC defaults are `killExisting: true` and `console: true` (device.md §7).
pub async fn app_launch(
    m: &DeviceManager,
    c: &Caller,
    p: pa::AppLaunchParams,
) -> Result<(DeviceHandle, LaunchedProcess)> {
    let h = handle_for(m, p.udid.as_deref(), c)?;
    let req = LaunchRequest {
        bundle_id: p.bundle_id,
        args: p.args,
        env: p.env,
        suspended: p.suspended.unwrap_or(false),
        kill_existing: p.kill_existing.unwrap_or(true),
        console: p.console.unwrap_or(true),
        via: LaunchVia::Auto,
    };
    let launched = h.launch(req).await?;
    Ok((h, launched))
}

pub async fn app_terminate(m: &DeviceManager, c: &Caller, p: pa::AppTerminateParams) -> Result<pa::AppTerminateResult> {
    let h = handle_for(m, p.udid.as_deref(), c)?;
    let pids = match (p.pid, p.bundle_id.as_deref()) {
        (Some(pid), _) => vec![pid],
        (None, Some(b)) => {
            let pids = crate::process::pids_of(&h, b).await?;
            if pids.is_empty() {
                return Err(russet_core::Error::new(codes::core::NOT_FOUND, format!("{b} is not running")));
            }
            pids
        }
        (None, None) => return Err(invalid_argument("app.terminate needs pid or bundleId")),
    };
    for pid in pids {
        h.signal(pid, 9).await?;
    }
    Ok(pa::AppTerminateResult {})
}

pub async fn process_list(m: &DeviceManager, p: pp::ProcessListParams) -> Result<pp::ProcessListResult> {
    let h = handle(m, p.udid.as_deref())?;
    Ok(pp::ProcessListResult { processes: h.processes().await?.iter().map(wire::process).collect() })
}

pub async fn process_signal(
    m: &DeviceManager,
    c: &Caller,
    p: pp::ProcessSignalParams,
) -> Result<pp::ProcessSignalResult> {
    let h = handle_for(m, p.udid.as_deref(), c)?;
    h.signal(p.pid, p.signal).await?;
    Ok(pp::ProcessSignalResult {})
}

/// `forward.open`: the handle stays with the caller (the daemon keeps it for the lease's life).
pub async fn forward_open(
    m: &DeviceManager,
    c: &Caller,
    p: pf::ForwardOpenParams,
) -> Result<(pf::ForwardOpenResult, ForwardHandle)> {
    let h = handle_for(m, p.udid.as_deref(), c)?;
    let target = wire::parse_forward_target(&p.target)?;
    let bind = match p.bind {
        None => None,
        Some(pf::ForwardBindParam { tcp: Some(t), unix: None }) => Some(crate::debug::parse_bind(&format!("tcp:{t}"))?),
        Some(pf::ForwardBindParam { tcp: None, unix: Some(u) }) => {
            Some(crate::debug::parse_bind(&format!("unix:{u}"))?)
        }
        Some(pf::ForwardBindParam { tcp: None, unix: None }) => None,
        Some(_) => return Err(invalid_argument("bind takes tcp or unix, not both")),
    };
    let fwd = crate::debug::open_forward_with(&h, target, bind).await?;
    let result =
        pf::ForwardOpenResult { lease_id: fwd.id.clone(), local: fwd.local.display(), url: fwd.local.lldb_url() };
    Ok((result, fwd))
}

/// The bind of `--bind tcp:ADDR|unix:PATH` as a DTO.
pub fn bind_param(b: &BindSpec) -> pf::ForwardBindParam {
    match b {
        BindSpec::Tcp(a) => pf::ForwardBindParam { tcp: Some(a.to_string()), unix: None },
        #[cfg(unix)]
        BindSpec::Unix(p) => pf::ForwardBindParam { tcp: None, unix: Some(p.display().to_string()) },
    }
}

pub fn forward_close(m: &DeviceManager, p: pf::ForwardCloseParams) -> Result<pf::ForwardCloseResult> {
    m.close_forward(&p.lease_id)?;
    Ok(pf::ForwardCloseResult {})
}

pub fn forward_list(m: &DeviceManager) -> pf::ForwardListResult {
    pf::ForwardListResult { forwards: m.forwards().iter().map(wire::forward).collect() }
}

/// The domain filter of `logs.subscribe`.
pub fn log_filter(sources: &[pl::LogsSource], f: &pl::LogFilter) -> LogFilter {
    LogFilter {
        pid: f.pid,
        bundle_id: f.bundle_id.clone(),
        process: f.process.clone(),
        subsystem: f.subsystem.clone(),
        category: f.category.clone(),
        min_level: f.min_level,
        contains: f.contains.clone(),
        sources: sources
            .iter()
            .map(|s| match s {
                pl::LogsSource::Oslog => LogSourceKind::OsLog,
                pl::LogsSource::Syslog => LogSourceKind::Syslog,
                pl::LogsSource::Stdout => LogSourceKind::Stdout,
            })
            .collect(),
    }
}

pub async fn logs_subscribe(m: &DeviceManager, p: &pl::LogsSubscribeParams) -> Result<LogStream> {
    let h = handle(m, p.udid.as_deref())?;
    h.logs(log_filter(&p.sources, &p.filter)).await
}

pub async fn logs_tail(m: &DeviceManager, p: pl::LogsTailParams) -> Result<pl::LogsTailResult> {
    let h = handle(m, p.udid.as_deref())?;
    let q = TailQuery {
        bundle_id: p.bundle_id,
        pid: p.pid,
        lines: p.lines,
        min_level: p.min_level,
        contains: p.contains,
        since: p.since,
        cursor: p.cursor,
        first_wait: None,
    };
    let r = crate::logs::tail(&h, q).await?;
    Ok(pl::LogsTailResult { entries: r.entries, truncated: r.truncated, cursor: r.cursor })
}

/// The manager behind an `Arc` (what russetd's `DeviceSlot` hands out).
pub type Manager = Arc<DeviceManager>;
