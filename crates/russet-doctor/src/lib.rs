// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet doctor`: the [`Check`] trait, the registry, the report model and the host checks (core §5.7, §11).
//!
//! Every domain crate exports `pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>>`; russetd's
//! `doctor_registry.rs` (a seam file) concatenates them (ARCHITECTURE §5.3).
//!
//! F0 stub (W1); W3 fills the bodies (CORE-12).

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use russet_core::error::{Error, Result, codes};

/// A check's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Skip,
}

/// Report grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Host,
    Daemon,
    Config,
    Secrets,
    Usbmux,
    Device,
    Toolchain,
    Account,
    Network,
    Platform,
}

/// What a check needs before it can run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Requires {
    pub daemon: bool,
    pub device: bool,
    pub network: bool,
    pub project: bool,
}

/// What a check runs against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckCx {
    pub project: Option<PathBuf>,
    pub udid: Option<String>,
    pub online: bool,
}

/// One check's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub status: Status,
    pub summary: String,
    pub detail: Option<String>,
    /// The next step for the user, when one exists.
    pub hint: Option<String>,
}

/// An environment check.
#[async_trait]
pub trait Check: Send + Sync {
    /// Stable id, for example `toolchain.linker`.
    fn id(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn category(&self) -> Category;
    fn requires(&self) -> Requires;
    async fn run(&self, cx: &CheckCx) -> CheckResult;
    async fn fix(&self, _cx: &CheckCx) -> Result<()> {
        Err(Error::new(codes::core::NOT_IMPLEMENTED, "no automatic fix"))
    }
}

/// Which checks to run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub checks: Vec<String>,
    pub categories: Vec<Category>,
}

/// One entry of a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    pub id: String,
    pub title: String,
    pub category: Category,
    pub result: CheckResult,
}

/// The result of `doctor.run`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorReport {
    pub checks: Vec<CheckOutcome>,
}

/// Registered checks.
#[derive(Default)]
pub struct Registry {
    checks: Vec<Arc<dyn Check>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(&mut self, c: Arc<dyn Check>) {
        self.checks.push(c);
    }
    /// The ids of the registered checks, in registration order.
    pub fn ids(&self) -> Vec<&'static str> {
        self.checks.iter().map(|c| c.id()).collect()
    }
    pub async fn run(&self, cx: &CheckCx, filter: &Filter) -> DoctorReport {
        todo!("W3 (CORE-12)")
    }
}

/// The host checks (W3).
pub fn checks() -> Vec<Arc<dyn Check>> {
    Vec::new()
}
