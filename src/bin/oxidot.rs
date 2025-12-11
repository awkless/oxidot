// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

use oxidot::{
    path::default_cluster_store_dir, Cluster, ClusterDefinition, ProgressBarAuthenticator, Store,
    WorkTreeAlias,
};

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use indicatif::{MultiProgress, ProgressBar};
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

    /// Number of jobs to run during dependency resoultion.
    pub jobs: Option<usize>,
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
        default_cluster_store_dir()?.join(format!("{}.git", opts.cluster_name)),
        definition,
    )?;

    Ok(())
}

async fn run_clone(opts: CloneOptions) -> Result<()> {
    let store = Store::new(default_cluster_store_dir()?)?;
    let path = default_cluster_store_dir()?.join(format!("{}.git", &opts.cluster_name));
    let bars = MultiProgress::new();
    let bar = bars.add(ProgressBar::no_length());
    let auth_bar = ProgressBarAuthenticator::new(bar);

    let cluster = Cluster::try_new_clone(&opts.url, path, auth_bar)?;
    store.insert(&opts.cluster_name, cluster);
    let resolved = store
        .resolve_dependencies(&opts.cluster_name, opts.jobs)
        .await?;
    store.with_clusters(|clusters| {
        let entries = resolved
            .into_iter()
            .map(|dep| clusters.get(&dep).unwrap())
            .collect::<Vec<_>>();
        for entry in entries {
            entry.deploy_default_rules()?;
        }

        Ok(())
    })
}

fn run_deploy(opts: DeployOptions) -> Result<()> {
    let store = Store::new(default_cluster_store_dir()?)?;
    store.with_clusters(|clusters| {
        let cluster = clusters
            .get(&opts.cluster_name)
            .ok_or(anyhow!("cluster {:?} not found", &opts.cluster_name))?;

        if !opts.sparsity_rules.is_empty() {
            return cluster.deploy_rules(opts.sparsity_rules);
        }

        if opts.all {
            return cluster.deploy_all();
        }

        if opts.default {
            return cluster.deploy_default_rules();
        }

        Ok(())
    })
}

fn run_undeploy(opts: UndeployOptions) -> Result<()> {
    let store = Store::new(default_cluster_store_dir()?)?;
    store.with_clusters(|clusters| {
        let cluster = clusters
            .get(&opts.cluster_name)
            .ok_or(anyhow!("cluster {:?} not found", &opts.cluster_name))?;

        if !opts.sparsity_rules.is_empty() {
            return cluster.undeploy_rules(opts.sparsity_rules);
        }

        if opts.all {
            return cluster.undeploy_all();
        }

        if opts.default {
            return cluster.undeploy_default_rules();
        }

        Ok(())
    })
}

fn run_list(opts: ListOptions) -> Result<()> {
    let store = Store::new(default_cluster_store_dir()?)?;

    if let Some(cluster_name) = opts.sparsity_rules {
        store.with_clusters(|clusters| {
            clusters
                .get(&cluster_name)
                .ok_or(anyhow!("cluster {:?} not found", &cluster_name))?
                .show_deploy_rules()
        })?;
    } else if let Some(cluster_name) = opts.files {
        store.with_clusters(|clusters| {
            clusters
                .get(&cluster_name)
                .ok_or(anyhow!("cluster {:?} not found", &cluster_name))?
                .show_tracked_files()
        })?;
    } else if opts.deployed {
        store.list_deployed();
    } else if opts.undeployed {
        store.list_undeployed();
    } else {
        store.list_fully();
    }

    Ok(())
}

fn run_remove(opts: RemoveOptions) -> Result<()> {
    let store = Store::new(default_cluster_store_dir()?)?;
    for name in &opts.cluster_name {
        store.remove(name)?;
    }

    Ok(())
}

fn run_git(opts: Vec<OsString>) -> Result<()> {
    // TODO: Allow for multiple clusters to be targeted.
    let target = opts[0].to_string_lossy().into_owned();
    let cluster =
        Cluster::try_new_open(default_cluster_store_dir()?.join(format!("{}.git", target)))?;
    cluster.gitcall_interactive(opts[1..].to_vec())?;

    Ok(())
}
