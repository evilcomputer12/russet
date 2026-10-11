// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask`: code generation and repository gates (ARCHITECTURE §4).
//!
//! Seam file: lead-only after W1 merges (PLAN §0 rule 2). Every subcommand of ARCHITECTURE §4 is registered here
//! and calls its own module file; owners fill their module and add flags to its `Args` (PLAN §3 [D 86]).

// xtask is a command-line tool: printing its report to stdout is its job.
#![allow(clippy::print_stdout)]

use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod ci;
mod ci_budget;
mod contracts;
mod disk_guard;
mod docs;
mod golden;
mod guard;
mod lab;
mod layers;
mod legal;
mod no_apple;
mod paths;
mod pins;
mod release;
mod schema;
mod sweep;
mod traces;
mod workstreams;

#[derive(Debug, Parser)]
#[command(name = "cargo xtask", bin_name = "cargo xtask", version, about = "Russet repository automation")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Regenerate the JSON schemas, the TypeScript client and docs/reference (W2)
    Schema(schema::Args),
    /// Generate and sign pins/stable.toml (W3 in Wave 1, W26 later)
    Pins(pins::Args),
    /// Check crate layering and layer F's permissive-licence rule (ARCHITECTURE §5.2)
    CheckLayers(layers::Args),
    /// Warn about edits outside a workstream's paths and about seam files (PLAN §0 rule 2)
    CheckPaths(paths::Args),
    /// Repository guards: apple-files, secrets, reuse, typos, public-terms (W8)
    Guard(guard::Args),
    /// Fail on Apple files in the tree (the `guard apple-files` scan; ARCHITECTURE §19)
    CheckNoApple(no_apple::Args),
    /// Check free disk space against the tiers before heavy work (W8)
    DiskGuard(disk_guard::Args),
    /// Run the CI gates locally: fmt, clippy, nextest, deny, guard, check-layers, schema drift (W8)
    Ci(ci::Args),
    /// Warn when the month's GitHub Actions usage passes 80 % of the included minutes (W8)
    CiBudget(ci_budget::Args),
    /// cargo nextest behind disk-guard (W8)
    Test(ci::TestArgs),
    /// cargo build behind disk-guard (W8)
    Build(ci::BuildArgs),
    /// Sweep the cargo target slots (W8)
    Sweep(sweep::Args),
    /// Device-in-the-loop suites; lane A only (W9)
    Lab(lab::Args),
    /// The golden corpus and oracle comparisons (W16)
    Golden(golden::Args),
    /// RDT trace tools: record, redact, drift (W15)
    Traces(traces::Args),
    /// Contract tests: schemas, CLI snapshots, MCP conformance (W15)
    Contracts(contracts::Args),
    /// Release engineering (W26)
    Release(release::Args),
    /// The legal release gate (W26)
    LegalCheck(legal::Args),
    /// The docs site and reference pages (W26)
    Docs(docs::Args),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Cmd::Schema(a) => schema::run(a),
        Cmd::Pins(a) => pins::run(a),
        Cmd::CheckLayers(a) => layers::run(a),
        Cmd::CheckPaths(a) => paths::run(a),
        Cmd::Guard(a) => guard::run(a),
        Cmd::CheckNoApple(a) => no_apple::run(a),
        Cmd::DiskGuard(a) => disk_guard::run(a),
        Cmd::Ci(a) => ci::run(a),
        Cmd::CiBudget(a) => ci_budget::run(a),
        Cmd::Test(a) => ci::run_test(a),
        Cmd::Build(a) => ci::run_build(a),
        Cmd::Sweep(a) => sweep::run(a),
        Cmd::Lab(a) => lab::run(a),
        Cmd::Golden(a) => golden::run(a),
        Cmd::Traces(a) => traces::run(a),
        Cmd::Contracts(a) => contracts::run(a),
        Cmd::Release(a) => release::run(a),
        Cmd::LegalCheck(a) => legal::run(a),
        Cmd::Docs(a) => docs::run(a),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// The error of a subcommand whose module is not filled yet.
fn not_implemented(command: &str, module: &str, owner: &str) -> anyhow::Error {
    anyhow::anyhow!("`cargo xtask {command}` is not implemented yet ({owner} owns xtask/src/{module})")
}
