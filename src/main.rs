// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::process::exit;
use tracing::error;
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Debug, Clone, Parser)]
#[command(
    about,
    override_usage = "\n  oxidot [options] <oxidot-command>\n  oxidot [options] [cluster]... <git-command>",
    subcommand_help_heading = "Commands",
    version
)]
struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    fn run(self) -> Result<()> {
        match self.command {
            Command::Clone(_opts) => run_clone(_opts),
        }
    }
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
    #[command(override_usage = "oxidot clone [options] <url>")]
    Clone(CloneOptions),
}

#[derive(Parser, Clone, Debug)]
#[command(author, about, long_about)]
struct CloneOptions {
    #[arg(value_name = "url")]
    pub url: String,
}

fn main() {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap();
    tracing_subscriber::registry().with(filter).init();

    if let Err(error) = run() {
        error!("{error:?}");
        exit(1);
    }

    exit(0)
}

fn run() -> Result<()> {
    Cli::parse().run()
}

fn run_clone(opts: CloneOptions) -> Result<()> {
    todo!();
}
