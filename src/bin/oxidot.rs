// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

use oxidot::{
    cluster::BranchTarget,
    config::{ClusterDefinition, WorkTreeAlias},
    path::{default_cluster_store_dir, home_dir},
    store::Store,
};

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
            Command::Status(opts) => run_status(opts),
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

    /// Show status information about cluster store.
    #[command(override_usage = "oxidot list [options]")]
    Status(StatusOptions),

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

    /// Target branch to use instead of the default branch.
    #[arg(short, long, value_name = "branch")]
    pub branch: Option<String>,

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

    /// Select branch to checkout.
    #[arg(short, long, value_name = "branch")]
    pub branch: Option<String>,
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
struct StatusOptions {
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
    pub cluster_names: Vec<String>,
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

fn run_init(opts: InitOptions) -> Result<()> {
    let mut definition = ClusterDefinition::default();
    definition.settings.description = match opts.description {
        Some(description) => description,
        None => "<put one sentence description here>".into(),
    };
    definition.settings.remote.url = match opts.url {
        Some(url) => url,
        None => "<put url to remote here>".into(),
    };
    definition.settings.remote.branch = match opts.branch {
        Some(branch) => Some(branch),
        None => None,
    };
    definition.settings.work_tree_alias = match opts.work_tree_alias {
        Some(path) => WorkTreeAlias::new(path),
        None => WorkTreeAlias::new(home_dir()?),
    };

    let store = Store::open(default_cluster_store_dir()?)?;
    store.init_cluster(opts.cluster_name, definition)?;

    Ok(())
}

async fn run_clone(opts: CloneOptions) -> Result<()> {
    let store = Store::open(default_cluster_store_dir()?)?;

    if let Some(branch) = opts.branch {
        store
            .clone_cluster(opts.cluster_name, opts.url, BranchTarget::Target(branch))
            .await?;
    } else {
        store
            .clone_cluster(opts.cluster_name, opts.url, BranchTarget::Default)
            .await?;
    };

    Ok(())
}

fn run_deploy(opts: DeployOptions) -> Result<()> {
    let store = Store::open(default_cluster_store_dir()?)?;
    store.use_cluster(opts.cluster_name, |cluster| {
        if opts.all {
            cluster.deploy_all()?;
        } else if opts.default {
            cluster.deploy_default_rules()?;
        } else {
            cluster.deploy_with_rules(opts.sparsity_rules)?;
        }

        Ok(())
    })?;

    Ok(())
}

fn run_undeploy(opts: UndeployOptions) -> Result<()> {
    let store = Store::open(default_cluster_store_dir()?)?;
    store.use_cluster(opts.cluster_name, |cluster| {
        if opts.all {
            cluster.undeploy_all()?;
        } else if opts.default {
            cluster.undeploy_default_rules()?;
        } else {
            cluster.undeploy_with_rules(opts.sparsity_rules)?;
        }

        Ok(())
    })?;

    Ok(())
}

fn run_status(opts: StatusOptions) -> Result<()> {
    let store = Store::open(default_cluster_store_dir()?)?;
    if let Some(cluster_name) = opts.sparsity_rules {
        store.tracked_files_status(cluster_name)?;
    } else if let Some(cluster_name) = opts.files {
        store.deploy_rules_status(cluster_name)?;
    } else if opts.deployed {
        store.deployed_only_status();
    } else if opts.undeployed {
        store.undeployed_only_status();
    } else {
        store.detailed_status();
    }

    Ok(())
}

fn run_remove(opts: RemoveOptions) -> Result<()> {
    let store = Store::open(default_cluster_store_dir()?)?;
    for cluster_name in opts.cluster_names {
        store.remove_cluster(cluster_name)?;
    }

    Ok(())
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
