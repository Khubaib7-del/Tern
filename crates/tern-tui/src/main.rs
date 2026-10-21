//! The `tern` command.
//!
//! Phase 2 ships `tern doctor`. `tern ask`, `tern run --json`, and the TUI are
//! planned for phase 3.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tern_agent::doctor::Status;

#[derive(Debug, Parser)]
#[command(
    name = "tern",
    version,
    about = "Tern: research and study, with sources you can check."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check that this machine can run Tern: keychain, data directory, disk, database.
    Doctor {
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Doctor { json } => run_doctor(json),
    }
}

fn run_doctor(json: bool) -> ExitCode {
    let report = tern_agent::doctor::run_on_this_machine();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("report serialises")
        );
    } else {
        println!("Tern {} on {}", report.version, report.os);
        println!();
        let width = report
            .checks
            .iter()
            .map(|c| c.name.len())
            .max()
            .unwrap_or(0);
        for c in &report.checks {
            println!("  {:<4}  {:<width$}  {}", mark(c.status), c.name, c.detail);
        }
        println!();
        println!(
            "{}",
            match report.overall() {
                Status::Ok => "Everything Tern needs is in place.",
                Status::Warn => "Tern can run, with the warnings above.",
                Status::Fail => "Tern cannot run yet. Fix the failed checks above.",
            }
        );
    }
    match report.overall() {
        Status::Fail => ExitCode::FAILURE,
        _ => ExitCode::SUCCESS,
    }
}

fn mark(s: Status) -> &'static str {
    match s {
        Status::Ok => "ok",
        Status::Warn => "warn",
        Status::Fail => "FAIL",
    }
}
