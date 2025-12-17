// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

use oxidot::{path::default_cluster_store_dir, store::Store};

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::{ffi::OsString, path::PathBuf, process::exit};
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
    async fn run(self) -> Result<()> {
        match self.command {
            Command::Init(opts) => run_init(opts),
            Command::Clone(opts) => run_clone(opts).await,
            Command::Deploy(opts) => run_deploy(opts),
            Command::Undeploy(opts) => run_undeploy(opts),
            Command::List(opts) => run_list(opts),
            Command::Remove(opts) => run_remove(opts),
            Command::Git(opts) => run_git(opts),
        }
    }
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
    /// Initialize new cluster.
    #[command(override_usage = "oxidot init [options] <cluster_name>")]
    Init(InitOptions),

    /// Clone existing cluster from remote.
    #[command(override_usage = "oxidot clone [options] <url>")]
    Clone(CloneOptions),

    /// Deploy tracked files in target file to work tree alias by sparisty rule.
    #[command(override_usage = "oxidot deploy [options] <cluster_name> [<sparsity_rules>]...")]
    Deploy(DeployOptions),

    /// Undeploy tracked files in target file from work tree alias by sparisty rule.
    #[command(override_usage = "oxidot undeploy [options] <cluster_name> [<sparsity_rules>]...")]
    Undeploy(UndeployOptions),

    /// List status information about cluster store.
    #[command(override_usage = "oxidot list [options]")]
    List(ListOptions),

    /// Remove cluster from cluster store.
    #[command(override_usage = "oxidot remove [options] <cluster_name>")]
    Remove(RemoveOptions),

    /// Run Git binary directly on target cluster.
    #[command(external_subcommand)]
    Git(Vec<OsString>),
}

#[derive(Parser, Clone, Debug)]
#[command(author, about, long_about)]
struct InitOptions {
    /// Name of cluster to register into cluster store.
    #[arg(value_name = "cluster_name")]
    pub cluster_name: String,

    /// Brief description of cluster entry.
    #[arg(short, long, value_name = "summary")]
    pub description: Option<String>,

    /// URL of remote to clone cluster from.
    #[arg(short, long, value_name = "url")]
    pub url: Option<String>,

    /// Path to work tree alias.
    #[arg(short, long, value_name = "path")]
    pub work_tree_alias: Option<PathBuf>,
}

#[derive(Parser, Clone, Debug)]
#[command(author, about, long_about)]
struct CloneOptions {
    /// Name of cluster to register into cluster store.
    #[arg(required = true, value_name = "name")]
    pub cluster_name: String,

    /// URL of remote to clone from.
    #[arg(required = true, value_name = "url")]
    pub url: String,
}

#[derive(Parser, Clone, Debug)]
#[command(author, about, long_about)]
struct DeployOptions {
    /// Name of cluster to deploy tracked files from.
    #[arg(required = true, value_name = "cluster_name")]
    pub cluster_name: String,

    /// List of sparsity rules to match tracked files to deploy.
    #[arg(group = "rules", value_name = "sparsity_rule")]
    pub sparsity_rules: Vec<String>,

    /// Deploy all tracked files to work tree alias.
    #[arg(short, long, group = "rules")]
    pub all: bool,

    /// Deploy default set of tracked files to work tree alias.
    #[arg(short, long, group = "rules")]
    pub default: bool,
}

#[derive(Parser, Clone, Debug)]
#[command(author, about, long_about)]
struct UndeployOptions {
    /// Name of cluster to deploy tracked files from.
    #[arg(required = true, value_name = "cluster_name")]
    pub cluster_name: String,

    /// List of sparsity rules to match tracked files to undeploy.
    #[arg(group = "rules", value_name = "sparsity_rule")]
    pub sparsity_rules: Vec<String>,

    /// Undeploy all tracked files to work tree alias.
    #[arg(short, long, group = "rules")]
    pub all: bool,

    /// Undeploy default set of tracked files to work tree alias.
    #[arg(short, long, group = "rules")]
    pub default: bool,
}

#[derive(Parser, Clone, Debug)]
#[command(author, about, long_about)]
struct ListOptions {
    /// List current sparsity rules of target cluster.
    #[arg(group = "target", short, long, value_name = "cluster_name")]
    pub sparsity_rules: Option<String>,

    #[arg(group = "target", short, long, value_name = "cluster_name")]
    pub files: Option<String>,

    /// List only deployed clusters.
    #[arg(group = "target", short, long)]
    pub deployed: bool,

    /// List only undeployed clusters.
    #[arg(group = "target", short, long)]
    pub undeployed: bool,
}

#[derive(Parser, Clone, Debug)]
#[command(author, about, long_about)]
struct RemoveOptions {
    /// Name of cluster to remove from cluster store.
    #[arg(required = true, value_name = "cluster_name")]
    pub cluster_name: Vec<String>,
}

#[tokio::main]
async fn main() {
    let layer = fmt::layer()
        .compact()
        .with_target(false)
        .with_timer(false)
        .without_time();
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap();
    tracing_subscriber::registry()
        .with(layer)
        .with(filter)
        .init();

    if let Err(error) = run().await {
        error!("{error:?}");
        exit(1);
    }

    exit(0)
}

async fn run() -> Result<()> {
    Cli::parse().run().await
}

fn run_init(__opts: InitOptions) -> Result<()> {
    todo!();
}

async fn run_clone(opts: CloneOptions) -> Result<()> {
    let store = Store::open(default_cluster_store_dir()?)?;
    store.clone_cluster(opts.cluster_name, opts.url).await?;

    Ok(())
}

fn run_deploy(_opts: DeployOptions) -> Result<()> {
    todo!();
}

fn run_undeploy(_opts: UndeployOptions) -> Result<()> {
    todo!();
}

fn run_list(_opts: ListOptions) -> Result<()> {
    todo!();
}

fn run_remove(_opts: RemoveOptions) -> Result<()> {
    todo!();
}

fn run_git(opts: Vec<OsString>) -> Result<()> {
    let store = Store::open(default_cluster_store_dir()?)?;
    let target = opts[0].to_string_lossy().into_owned();
    store.use_cluster(target, |cluster| {
        cluster.gitcall_interactive(opts[1..].to_vec())?;
        Ok(())
    })?;
    Ok(())
}
