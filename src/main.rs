// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

use oxidot::{cluster_store_dir, Cluster, ClusterDefinition, WorkTreeAlias};

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::{path::PathBuf, process::exit};
use tracing::error;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

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
            Command::Init(opts) => run_init(opts),
            Command::Clone(_opts) => run_clone(_opts),
        }
    }
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
    #[command(override_usage = "oxidot init [options] <cluster_name>")]
    Init(InitOptions),

    #[command(override_usage = "oxidot clone [options] <url>")]
    Clone(CloneOptions),
}

#[derive(Parser, Clone, Debug)]
#[command(author, about, long_about)]
struct InitOptions {
    #[arg(value_name = "cluster_name")]
    pub name: String,

    #[arg(short, long, value_name = "summary")]
    pub description: Option<String>,

    #[arg(short, long, value_name = "url")]
    pub url: Option<String>,

    #[arg(short, long, value_name = "path")]
    pub work_tree_alias: Option<PathBuf>,
}

#[derive(Parser, Clone, Debug)]
#[command(author, about, long_about)]
struct CloneOptions {
    #[arg(value_name = "url")]
    pub url: String,
}

fn main() {
    let layer = fmt::layer().compact();
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap();
    tracing_subscriber::registry().with(layer).with(filter).init();

    if let Err(error) = run() {
        error!("{error:?}");
        exit(1);
    }

    exit(0)
}

fn run() -> Result<()> {
    Cli::parse().run()
}

fn run_init(opts: InitOptions) -> Result<()> {
    let mut definition = ClusterDefinition::default();
    definition.settings.description = match opts.description {
        Some(description) => description,
        None => "<put one sentence description here>".into(),
    };
    definition.settings.url = match opts.url {
        Some(url) => url,
        None => "<put url to remote here>".into(),
    };
    definition.settings.work_tree_alias = match opts.work_tree_alias {
        Some(path) => WorkTreeAlias::new(path),
        None => WorkTreeAlias::try_default()?,
    };

    let _ = Cluster::try_new_init(
        cluster_store_dir()?.join(format!("{}.git", opts.name)),
        definition,
    )?;

    Ok(())
}

fn run_clone(_opts: CloneOptions) -> Result<()> {
    todo!();
}
