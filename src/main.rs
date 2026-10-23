mod app;

use anyhow::{Context, Result};
use clap::Parser;
use std::{ffi::OsString, path::PathBuf};
use vibeterm::config::{self, Config};

#[derive(Debug, Parser)]
#[command(version, about = "A native GPU terminal for Linux")]
pub struct Args {
    /// Start the shell in this directory.
    #[arg(short = 'd', long)]
    directory: Option<PathBuf>,
    /// Configuration file (default: $XDG_CONFIG_HOME/vibeterm/config.toml).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Keep the window open after the command exits.
    #[arg(long)]
    hold: bool,
    /// Print GPU and frame-count diagnostics, never terminal contents.
    #[arg(long)]
    diagnostics: bool,
    /// Validate configuration without opening a window.
    #[arg(long)]
    check_config: bool,
    /// Run a program with its exact arguments; must be the last option.
    #[arg(short = 'e', long, num_args = 1.., allow_hyphen_values = true)]
    command: Vec<OsString>,
}

fn run() -> Result<()> {
    let mut args = Args::parse();
    let path = args.config.clone().unwrap_or_else(config::path);
    let config = Config::load(&path)?;
    if args.check_config {
        println!("Configuration valid: {}", path.display());
        return Ok(());
    }
    if let Some(dir) = &args.directory {
        let dir = dir
            .canonicalize()
            .context("Cannot resolve working directory")?;
        anyhow::ensure!(dir.is_dir(), "Working directory must be a directory");
        args.directory = Some(dir);
    }
    app::run(args, config, path)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("vibeterm: {error:#}");
        std::process::exit(1);
    }
}
