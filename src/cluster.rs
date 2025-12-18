// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

//! Cluster domain representation.
//!
//! A __cluster__ is a bare-alias repository whose contents can be deployed to a
//! target working tree alias.
//!
//! # Bare-Alias Repositories
//!
//! All clusters in oxidot are considered __bare-alias__ repositories. Although
//! bare repositories lack a working tree by definition, Git allows users to
//! force a working tree by designating a directory as an alias for a working
//! tree using the "--work-tree" argument. This functionality enables us to
//! define a bare repository where the Git directory, and the alias working tree
//! are kept separate. This unique feature allows us to treat an entire
//! directory as a Git repository without needing to initialize it as one.
//!
//! This technique does not really have a standard name despite being a common
//! method to manage dotfile configurations through Git. Se we call it the
//! __bare-alias technique__. Hence, the term _bare-alias_ repository!
//!
//! # Cluster Components
//!
//! A cluster mainly contains two basic things: tracked files, and a
//! __cluster definition__. Tracked files are the various dotfile configurations
//! that the cluster needs to keep track of, and deploy to its target work tree
//! alias. However, the cluster definition specifies the actual configuration
//! settings and dependencies of the cluster itself.
//!
//! ## Cluster Definition
//!
//! A cluster definition is a special tracked file that specifies configuration
//! settings that are needed to determine how Oxidot should treat a given
//! cluster, e.g., give basic description of the cluster, specify the work tree
//! alias to use, etc.  The cluster definition can also be used to list other
//! clusters as dependencies of the current cluster. These dependencies will be
//! deployed along side their parent cluster.
//!
//! All clusters must contain a valid definition file at the top-level named
//! "cluster.toml". If this file cannot be found, then the cluster is considered
//! to be invalid, i.e., not a true cluster. Thus, all clusters must be
//! bare-alias and contain a cluster definition file to be considered a valid
//! cluster.
//!
//! # Cluster Deployment
//!
//! Oxidot performs cluster deployment through Git's sparse checkout feature.
//! The user must supply a valid listing of spasrity rules that match the
//! tracked files that they want deployed to a any given cluster's work tree
//! alias. Sparse checkout allows Oxidot's cluster deployment feature to
//! properly deploy tracked files without touching the commit history or
//! index of the cluster itself. This also simplfies deployment logic, because
//! a good portion of it is offloaded to Git.
//!
//! # See Also
//!
//! 1. [ArchWiki - dotfiles](https://wiki.archlinux.org/title/Dotfiles#Tracking_dotfiles_directly_with_Git)
//! 2. [`ClusterDefinition`]
//! 3. [`sparse`]

pub mod deploy;
pub mod sparse;

use crate::{
    cluster::{
        deploy::{Deployment, Git2Deployer},
        sparse::{InvertedGitignore, SparsityDrafter},
    },
    config::ClusterDefinition,
};

use auth_git2::{GitAuthenticator, Prompter};
use git2::{build::RepoBuilder, Config, FetchOptions, RemoteCallbacks, Repository};
use indicatif::{ProgressBar, ProgressStyle};
use inquire::{Password, Text};
use std::{ffi::OsString, path::{PathBuf, Path}, time};
use tracing::{debug, info, instrument, warn};

/// A basic cluster.
///
/// A __cluster__ is a bare-alias repository whose contents can be deployed to a
/// target working tree alias. Through a cluster, the user can keep track of
/// essential files in a target directory labeled as a work tree alias, without
/// needing to initialize it as a Git repository. Tracked files can be deployed
/// or undeployed to the work tree alias at will.
#[derive(Debug)]
pub struct Cluster<D = Git2Deployer>
where
    D: Deployment,
{
    pub(crate) definition: ClusterDefinition,
    pub(crate) deployer: D,
}

impl<D> Cluster<D>
where
    D: Deployment,
{
    /// Construct new cluster.
    pub fn new(definition: ClusterDefinition, deployer: D) -> Self {
        Self {
            definition,
            deployer,
        }
    }

    /// Deploy tracked files to work tree alias that match sparsity rules.
    ///
    /// Adds target rules to match tracked files, and updates cluster's index.
    ///
    /// # Errors
    ///
    /// - Return [`ClusterError::Deploy`] if deployment logic fails.
    pub fn deploy_with_rules(
        &self,
        rules: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<()> {
        Ok(self
            .deployer
            .deploy_with_rules(&self.definition.settings.work_tree_alias, rules)?)
    }

    /// Undeploy tracked files from work tree alias that match sparsity rules.
    ///
    /// Removes target rules from sparse checkout configuration file, and
    /// applies updates to cluster's index.
    ///
    /// # Errors
    ///
    /// - Return [`ClusterError::Deploy`] if deployment logic fails.
    pub fn undeploy_with_rules(
        &self,
        rules: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<()> {
        Ok(self
            .deployer
            .undeploy_with_rules(&self.definition.settings.work_tree_alias, rules)?)
    }

    /// Deploy tracked files based on default set of spasrity rules provided by
    /// cluster definition.
    ///
    /// Clears current sparsity rules, and replaces them with default
    /// sparsity rules from cluster definition.
    ///
    /// # Errors
    ///
    /// - Return [`ClusterError::Deploy`] if deployment logic fails.
    pub fn deploy_default_rules(&self) -> Result<()> {
        let work_tree_alias = &self.definition.settings.work_tree_alias;
        if let Some(default) = &self.definition.settings.include {
            self.deployer.undeploy_all(work_tree_alias)?;
            self.deployer.deploy_with_rules(work_tree_alias, default)?;
        }

        Ok(())
    }

    /// Undeploy tracked files based on default set of spasrity rules provided
    /// by cluster definition.
    ///
    /// Removes only the rules that match the default sparsity rules provided
    /// by the cluster definition.
    ///
    /// # Errors
    ///
    /// - Return [`ClusterError::Deploy`] if deployment logic fails.
    pub fn undeploy_default_rules(&self) -> Result<()> {
        let work_tree_alias = &self.definition.settings.work_tree_alias;
        if let Some(default) = &self.definition.settings.include {
            self.deployer
                .undeploy_with_rules(work_tree_alias, default)?;
        }

        Ok(())
    }

    /// Deploy all tracked files of cluster to work tree alias.
    ///
    /// Replaces entire sparsity rule set with one rule: "/*". Applies this
    /// new and only rule to cluster's index.
    ///
    /// # Errors
    ///
    /// - Return [`ClusterError::Deploy`] if deployment logic fails.
    pub fn deploy_all(&self) -> Result<()> {
        Ok(self
            .deployer
            .deploy_all(&self.definition.settings.work_tree_alias)?)
    }

    /// Undeploy all tracked files of cluster from work tree alias.
    ///
    /// Simply clears entire sparsity rule set, and applies this change to the
    /// cluster's index.
    ///
    /// # Panics
    ///
    /// - May panic if spasrity rule parsing fails.
    ///
    /// # Errors
    ///
    pub fn undeploy_all(&self) -> Result<()> {
        Ok(self
            .deployer
            .undeploy_all(&self.definition.settings.work_tree_alias)?)
    }

    /// List current set of sparsity rules used for deployment.
    ///
    /// # Errors
    ///
    /// - Return [`ClusterError::Deploy`] if deployment logic fails.
    pub fn list_deploy_rules(&self) -> Result<Vec<String>> {
        Ok(self.deployer.list_deploy_rules()?)
    }

    /// List currently tracked files of cluster.
    ///
    /// # Errors
    ///
    /// - Return [`ClusterError::Deploy`] if deployment logic fails.
    pub fn list_tracked_files(&self) -> Result<Vec<PathBuf>> {
        Ok(self.deployer.list_tracked_files()?)
    }

    /// Check if cluster has deployed tracked files to work tree alias.
    ///
    /// Performs a first occurance search through each tracked file in the
    /// cluster such that the tracked file that matches a sparsity rule, and
    /// exists in the cluster's work tree alias, means that the cluster is
    /// deployed. Otherwise, the cluster is not deployed.
    ///
    /// # Panics
    ///
    /// - May panic if spasrity rule parsing fails.
    pub fn is_deployed(&self) -> bool {
        self.deployer
            .is_deployed(&self.definition.settings.work_tree_alias)
    }

    /// Interact with cluster directly through Git via current process.
    ///
    /// Preserves consistency between sparsity rules and index when caller
    /// uses commands like git-add, git-rm, etc. Blocks current process to allow
    /// for direct interaction with cluster through Git.
    ///
    /// # Errors
    ///
    /// - Return [`ClusterError::Deploy`] if deployment logic fails.
    pub fn gitcall_interactive(
        &self,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<()> {
        Ok(self
            .deployer
            .gitcall_interactive(&self.definition.settings.work_tree_alias, args)?)
    }

    /// Interact with cluster directly through Git via external process.
    ///
    /// Does not block current process. Instead the system call is made via
    /// external process whose output to stdout and stderr is returned
    /// together as a [`String`].
    ///
    /// # Errors
    ///
    /// - Return [`ClusterError::Deploy`] if deployment logic fails.
    pub fn gitcall_non_interactive(
        &self,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<String> {
        Ok(self
            .deployer
            .gitcall_non_interactive(&self.definition.settings.work_tree_alias, args)?)
    }
}

/// Layer of indirection for cluster access.
pub trait ClusterAccess: Send + Sync + 'static {
    /// Initialize a new cluster.
    fn try_init(path: impl AsRef<Path>, definition: ClusterDefinition) -> Result<Cluster>;

    /// Open existing cluster.
    fn try_open(path: impl AsRef<Path>) -> Result<Cluster>;

    /// Clone existing cluster from remote repository.
    fn try_clone(url: impl AsRef<str>, path: impl AsRef<Path>, bar: ProgressBar) -> Result<Cluster>;
}

/// Cluster access through libgit2.
#[derive(Debug, Default)]
pub struct Git2Cluster;

impl ClusterAccess for Git2Cluster {
    /// Initialize a new cluster.
    ///
    /// Initializes a new cluster at target path based on a given cluster
    /// definition. That cluster definition will be staged and committed
    /// directly into the cluster's commit history as the initial commit.
    ///
    /// # Errors
    ///
    /// - Return [`ClusterError::Git2`] if libgit2 operations fail.
    /// - Return [`ClusterError::Deploy`] if deployment logic fails.
    #[instrument(skip(path, definition), level = "debug")]
    fn try_init(path: impl AsRef<Path>, definition: ClusterDefinition) -> Result<Cluster> {
        info!("initialize new cluster: {:?}", path.as_ref().display());
        let repository = Repository::init_bare(path.as_ref())?;
        let matcher = InvertedGitignore::new();
        let sparsity = SparsityDrafter::new(path.as_ref(), matcher)?;
        let deployer = Git2Deployer::new(repository, sparsity)?;

        let cluster = Cluster {
            definition,
            deployer,
        };
        let contents = &cluster.definition.to_string();
        info!(
            "stage and commit the following cluster definition:\n{}",
            contents
        );

        cluster
            .deployer
            .stage_and_commit("cluster.toml", contents, "chore: add cluster.toml")?;
        cluster.gitcall_non_interactive(["checkout"])?;

        Ok(cluster)
    }

    /// Open existing cluster.
    ///
    /// Opens cluster at target path, and loads up its cluster definition at the
    /// top-level.
    ///
    /// # Errors
    ///
    /// - Return [`ClusterError::Git2`] if libgit2 operations fail.
    /// - Return [`ClusterError::Config`] if cluster definition parsing fails.
    /// - Return [`ClusterError::Deploy`] if deployment logic fails.
    #[instrument(skip(path), level = "debug")]
    fn try_open(path: impl AsRef<Path>) -> Result<Cluster> {
        debug!("open cluster: {:?}", path.as_ref().display());
        let repository = Repository::open(path.as_ref())?;
        let matcher = InvertedGitignore::new();
        let sparsity = SparsityDrafter::new(path.as_ref(), matcher)?;
        let deployer = Git2Deployer::new(repository, sparsity)?;
        let definition = deployer.cat_file("cluster.toml")?.parse()?;

        Ok(Cluster {
            definition,
            deployer,
        })
    }

    /// Clone existing cluster from remote repository.
    ///
    /// Clones existing cluster from remote repository through target URL to
    /// target path. The progress of the clone is displayed through a progress
    /// bar. The cluster definition is then extracted at the top-level after
    /// the cloning is done.
    ///
    /// If any credentials are required for the clone to continue is required,
    /// then the user will be prompted for that information accordingly. The
    /// progress bar will be blocked for user input.
    ///
    /// # Errors
    ///
    /// - Return [`ClusterError::Git2`] if libgit2 operations fail.
    /// - Return [`ClusterError::Config`] if cluster definition parsing fails.
    /// - Return [`ClusterError::Deploy`] if deployment logic fails.
    fn try_clone(url: impl AsRef<str>, path: impl AsRef<Path>, bar: ProgressBar) -> Result<Cluster> {
        let style = ProgressStyle::with_template(
            "{elapsed_precise:.green}  {msg:<50}  [{wide_bar:.yellow/blue}]",
        )?
        .progress_chars("-Cco.");
        bar.set_style(style);
        bar.set_message(url.as_ref().to_string());
        bar.enable_steady_tick(std::time::Duration::from_millis(100));

        let prompter = IndicatifPrompter::new(bar);
        let authenticator = GitAuthenticator::default().set_prompter(prompter.clone());
        let config = Config::open_default()?;

        let mut throttle = time::Instant::now();
        let mut rc = RemoteCallbacks::new();
        rc.credentials(authenticator.credentials(&config));
        rc.transfer_progress(|progress| {
            let stats = progress.to_owned();
            let bar_size = stats.total_objects() as u64;
            let bar_pos = stats.received_objects() as u64;
            if throttle.elapsed() > time::Duration::from_millis(10) {
                throttle = time::Instant::now();
                prompter.bar.set_length(bar_size);
                prompter.bar.set_position(bar_pos);
            }
            true
        });

        let mut fo = FetchOptions::new();
        fo.remote_callbacks(rc);
        let repository = RepoBuilder::new()
            .bare(true)
            .fetch_options(fo)
            .clone(url.as_ref(), path.as_ref())?;

        let matcher = InvertedGitignore::new();
        let sparsity = SparsityDrafter::new(path.as_ref(), matcher)?;
        let deployer = Git2Deployer::new(repository, sparsity)?;
        let definition = deployer.cat_file("cluster.toml")?.parse()?;

        Ok(Cluster {
            definition,
            deployer,
        })
    }
}

/// Git2 authentication prompter for progress bar.
#[derive(Debug, Clone)]
pub struct IndicatifPrompter {
    pub(crate) bar: ProgressBar,
}

impl IndicatifPrompter {
    /// Construct new progress bar authenticator.
    pub fn new(bar: ProgressBar) -> Self {
        Self { bar }
    }
}

impl Prompter for IndicatifPrompter {
    #[instrument(skip(self, url, _config), level = "debug")]
    fn prompt_username_password(
        &mut self,
        url: &str,
        _config: &git2::Config,
    ) -> Option<(String, String)> {
        info!("authentication required at {url}");
        self.bar.suspend(|| -> Option<(String, String)> {
            let username = Text::new("username").prompt().unwrap();
            let password = Password::new("password")
                .without_confirmation()
                .prompt()
                .unwrap();
            Some((username, password))
        })
    }

    #[instrument(skip(self, username, url, _config), level = "debug")]
    fn prompt_password(
        &mut self,
        username: &str,
        url: &str,
        _config: &git2::Config,
    ) -> Option<String> {
        info!("authentication required at {url} for user {username}");
        self.bar.suspend(|| -> Option<String> {
            let password = Password::new("password")
                .without_confirmation()
                .prompt()
                .unwrap();
            Some(password)
        })
    }

    #[instrument(skip(self, ssh_key_path, _config), level = "debug")]
    fn prompt_ssh_key_passphrase(
        &mut self,
        ssh_key_path: &Path,
        _config: &git2::Config,
    ) -> Option<String> {
        info!(
            "authentication required with ssh key at {}",
            ssh_key_path.display()
        );
        self.bar.suspend(|| -> Option<String> {
            let password = Password::new("password")
                .without_confirmation()
                .prompt()
                .unwrap();
            Some(password)
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClusterError {
    /// Deployment logic fails.
    #[error(transparent)]
    Deployment(#[from] crate::cluster::deploy::DeployError),

    /// Sparse checkout configuration file manipulation fails.
    #[error(transparent)]
    Sparse(#[from] crate::cluster::sparse::SparseError),

    /// Cluster definition parsing fails.
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),

    /// Style template cannot be set for progress bars.
    #[error(transparent)]
    IndicatifStyleTemplate(#[from] indicatif::style::TemplateError),

    /// Operations from libgit2 fail.
    #[error(transparent)]
    Git2(#[from] git2::Error),
}

/// Friendly result alias :3
type Result<T, E = ClusterError> = std::result::Result<T, E>;
