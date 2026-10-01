mod archive;
mod config;
mod filesystem;
mod sources;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::Config;
use std::{path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(long)]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture complete native transcripts locally.
    Collect,
    /// Show the last local collection report, without collecting.
    Status,
    /// Validate configuration and paths without creating files.
    CheckConfig,
}

fn run() -> Result<bool> {
    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;
    match cli.command {
        Command::CheckConfig => {
            println!("{}", serde_json::to_string_pretty(&config)?);
            Ok(true)
        }
        Command::Collect => {
            let report = archive::collect(&config)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(report.errors == 0)
        }
        Command::Status => {
            println!(
                "{}",
                serde_json::to_string_pretty(&archive::status(&config)?)?
            );
            Ok(true)
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("agent-echo: {error:#}");
            ExitCode::FAILURE
        }
    }
}
