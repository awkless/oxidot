// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-FileCopyrightText: 2024-2025 Eric Urban <hydrogen18@gmail.com>
// SPDX-License-Identifier: MIT

#![warn(
    clippy::complexity,
    clippy::correctness,
    missing_debug_implementations,
    rust_2021_compatibility
)]
#![doc(issue_tracker_base_url = "https://github.com/awkless/oxidot/issues")]

pub mod cluster;
pub mod config;
pub mod path;

use crate::cluster::sparse::{InvertedGitignore, SparsityDrafter};
use crate::config::ClusterDefinition;

use anyhow::{anyhow, Context, Result};
use auth_git2::{GitAuthenticator, Prompter};
use futures::{stream, StreamExt};
use git2::{
    build::RepoBuilder, Config, FetchOptions, IndexEntry, IndexTime, ObjectType, RemoteCallbacks,
    Repository,
};
use ignore::gitignore::GitignoreBuilder;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use inquire::{Password, Text};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::{OsStr, OsString},
    fmt,
    fs::remove_dir_all,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, MutexGuard},
    time,
};
use tracing::{debug, info, instrument, warn};

/// Cluster store management.
///
/// Oxidot keeps track of available clusters through a __cluster store__. The
/// cluster store is just a basic external directory where all of the clusters
/// are kept for easy access.
///
/// # Naming Conventions
///
/// Each entry in the cluster store comes with a ".git" extension. The
/// name of each cluster is just the name of directory stored in the cluster
/// store itself. Thus, a cluster named "editor" will have a corresponding
/// bare-alias repository in the cluster store as "editor.git".
///
/// Oxidot only considers the top-level of the cluster store when processing
/// cluster data. Thus, it is not possible for oxidot to detect nested clusters.
///
/// # Cluster Store Location
///
/// The cluster store can be placed pretty much anywhere the caller wants within
/// their filesystem. At least when it comes to this API. Typically, as a
/// default path, oxidot idiomatically prefers `$XDG_DATA_HOME/oxidot-store`.
/// However, that is definitely a preference, and not a hard coded rule.
///
/// # See Also
///
/// 1. [`Cluster`](struct.Cluster)
#[derive(Debug)]
pub struct Store {
    store_path: PathBuf,
    clusters: Arc<Mutex<HashMap<String, Cluster>>>,
}

impl Store {
    /// Construct new cluster store manager.
    ///
    /// Will treat target directory as a cluster store. All clusters within
    /// that path will be opened for management and manipulation.
    ///
    /// # Errors
    ///
    /// Will fail if any cluster cannot be opened at target path for whatever
    /// reason.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let store_path = path.into();
        let pattern = store_path.join("*.git").to_string_lossy().into_owned();
        let mut clusters = HashMap::new();
        for entry in glob::glob(pattern.as_str())? {
            // INVARIANT: The name of a cluster is the directory name minus the .git extension.
            let path = entry?;
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            clusters.insert(name, Cluster::try_new_open(path)?);
        }

        // TODO: Add checks for valid cluster store structure at some point.
        let store = Self {
            store_path,
            clusters: Arc::new(Mutex::new(clusters)),
        };

        Ok(store)
    }

    /// Insert a cluster into the store.
    ///
    /// Returns `None` if and only if the cluster was new to the store. If the
    /// given cluster already exists in the store, then the old cluster will be
    /// returned after the new cluster takes its place in the store. The name
    /// of the cluster is never updated.
    pub fn insert(&self, name: impl Into<String>, cluster: Cluster) -> Option<Cluster> {
        let mut clusters = self.clusters();
        clusters.insert(name.into(), cluster)
    }

    /// Remove a cluster from the store.
    ///
    /// Will remove the entire cluster from the cluster store path, and removes
    /// the cluster from the store manager itself. The newly removed cluster
    /// entry is returned.
    ///
    /// # Errors
    ///
    /// - Will fail if cluster cannot be removed from the cluster store path.
    /// - Will fail if cluster does not exist in the store.
    #[instrument(skip(self, name), level = "debug")]
    pub fn remove(&self, name: impl AsRef<str>) -> Result<Cluster> {
        info!("remove {:?} from cluster store", name.as_ref());
        let cluster_path = self.store_path.join(format!("{}.git", name.as_ref()));
        let mut clusters = self.clusters();

        let cluster = clusters
            .remove(name.as_ref())
            .ok_or(anyhow!("cluster {:?} not in store", name.as_ref()))?;
        cluster.undeploy_all()?;

        remove_dir_all(&cluster_path)
            .with_context(|| anyhow!("failed to remove {:?} from store", name.as_ref()))?;

        Ok(cluster)
    }

    pub fn with_clusters<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&HashMap<String, Cluster>) -> R,
    {
        let clusters = self.clusters();
        f(&clusters)
    }

    /// List all available clusters with full details.
    #[instrument(skip(self), level = "debug")]
    pub fn list_fully(&self) {
        let clusters = self.clusters();
        if clusters.is_empty() {
            warn!("cluster store is empty");
            return;
        }

        let mut listing = String::new();
        for (name, entry) in clusters.iter() {
            let status = if entry.is_deployed() {
                "[  deployed]"
            } else {
                "[undeployed]"
            };

            let data = format!(
                "{} {} -> {}\n",
                status, name, entry.definition.settings.work_tree_alias
            );
            listing.push_str(data.as_str());
        }

        info!("all available clusters:\n{}", listing);
    }

    /// List currently deployed clusters.
    #[instrument(skip(self), level = "debug")]
    pub fn list_deployed(&self) {
        let clusters = self.clusters();
        if clusters.is_empty() {
            warn!("cluster store is empty");
            return;
        }

        let mut listing = String::new();
        for (name, entry) in clusters.iter() {
            if entry.is_deployed() {
                let data = format!(
                    "{} -> {}\n",
                    name, entry.definition.settings.work_tree_alias
                );
                listing.push_str(data.as_str());
            }
        }

        info!("all deployed clusters:\n{}", listing);
    }

    /// List currently undeployed clusters.
    #[instrument(skip(self), level = "debug")]
    pub fn list_undeployed(&self) {
        let clusters = self.clusters();
        if clusters.is_empty() {
            warn!("cluster store is empty");
            return;
        }

        let mut listing = String::new();
        for (name, entry) in clusters.iter() {
            if !entry.is_deployed() {
                let data = format!(
                    "{} -> {}\n",
                    name, entry.definition.settings.work_tree_alias
                );
                listing.push_str(data.as_str());
            }
        }

        info!("all undeployed clusters:\n{}", listing);
    }

    /// Make sure that a target cluster's dependencies exist in the store.
    ///
    /// Goes through the dependency listing (if any) of a cluster and goes
    /// through the process of cloning any dependencies that are missing in
    /// the cluster store. Skips over dependencies that already exist. Returns
    /// the names of the clusters that were missing.
    ///
    /// # Errors
    ///
    /// Will fail if any dependency cannot be properly cloned.
    pub async fn resolve_dependencies(
        &self,
        cluster_name: impl AsRef<str>,
        jobs: Option<usize>,
    ) -> Result<Vec<String>> {
        let unresolved = self.find_unresolved_dependencies(cluster_name.as_ref())?;
        if unresolved.is_empty() {
            return Ok(Vec::new());
        }

        let bars = MultiProgress::new();
        let results = Arc::new(Mutex::new(VecDeque::new()));
        let store_path = self.store_path.clone();
        let clusters = self.clusters.clone();

        stream::iter(unresolved.clone())
            .for_each_concurrent(jobs, |dep_name| {
                let results = results.clone();
                let bars = bars.clone();
                let store_path = store_path.clone();
                let clusters = clusters.clone();

                async move {
                    let bar = bars.add(ProgressBar::no_length());
                    let clusters_arc_inner = clusters.clone();
                    let store_path_inner = store_path.clone();
                    let bar_inner = bar.clone();
                    let dep_name_inner = dep_name.clone();

                    let result = tokio::spawn(async move {
                        Self::clone_missing_cluster(
                            &clusters_arc_inner,
                            &store_path_inner,
                            bar_inner,
                            &dep_name_inner,
                        )
                    })
                    .await;

                    let mut guard = results.lock().unwrap();
                    guard.push_back(
                        result.map_err(|err| anyhow!("Failed to clone {dep_name:?}: {err:?}")),
                    );
                    drop(guard);
                    // TODO: Is it okay to finish and clear the bar here?
                    bar.finish_and_clear();
                }
            })
            .await;

        // INVARIANT: Collect and report failures encountered.
        let results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
        let resolved = results.into_iter().flatten().collect::<Result<Vec<_>>>()?;
        let resolved_names = resolved
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();

        // INVARIANT: Insert resolved cluster dependencies into store.
        for (name, entry) in resolved {
            clusters.lock().unwrap().insert(name.clone(), entry);
        }

        Ok(resolved_names)
    }

    fn find_unresolved_dependencies(&self, cluster_name: &str) -> Result<Vec<String>> {
        let clusters = self.clusters();
        let mut unresolved = Vec::new();
        let mut visited = HashSet::new();
        let mut stack = VecDeque::new();
        stack.push_back(cluster_name.to_string());

        while let Some(current) = stack.pop_back() {
            if !visited.insert(current.clone()) {
                continue;
            }

            if !clusters.contains_key(&current) {
                unresolved.push(current.clone());
            }

            if let Some(cluster) = clusters.get(&current) {
                if let Some(deps) = &cluster.definition.dependencies {
                    for dep in deps {
                        if !visited.contains(&dep.name) {
                            stack.push_back(dep.name.clone());
                        }
                    }
                }
            }
        }

        Ok(unresolved)
    }

    fn clone_missing_cluster(
        clusters_arc: &Arc<Mutex<HashMap<String, Cluster>>>,
        store_path: &Path,
        bar: ProgressBar,
        name: &str,
    ) -> Result<(String, Cluster)> {
        let clusters = clusters_arc.lock().unwrap();
        let dep_info = clusters
            .values()
            .flat_map(|c| c.definition.dependencies.iter().flatten())
            .find(|d| d.name == name)
            .ok_or_else(|| anyhow!("dependency {:?} not declared", name))?;

        let url = dep_info.url.clone();
        drop(clusters); // Release lock before cloning

        let style = ProgressStyle::with_template(
            "{elapsed_precise:.green}  {msg:<50}  [{wide_bar:.yellow/blue}]",
        )
        .unwrap()
        .progress_chars("-Cco.");
        bar.set_style(style);
        bar.set_message(name.to_string());
        bar.enable_steady_tick(std::time::Duration::from_millis(100));
        let auth_bar = ProgressBarAuthenticator::new(bar.clone());
        let path = store_path.join(format!("{}.git", name));
        let cluster = Cluster::try_new_clone(&url, path, auth_bar)?;

        Ok((name.to_string(), cluster))
    }

    #[inline]
    fn clusters(&self) -> MutexGuard<'_, HashMap<String, Cluster>> {
        self.clusters.lock().unwrap()
    }
}

/// Cluster of dotfiles.
///
/// A __cluster__ is a bare repository whose contents can be deployed to a
/// target working tree alias.
///
/// # Bare-Alias Repositories
///
/// All clusters in oxidot are considered __bare-alias__ repositories. Although
/// bare repositories lack a working tree by definition, Git allows users to
/// force a working tree by designating a directory as an alias for a working
/// tree using the "--work-tree" argument. This functionality enables us to
/// define a bare repository where the Git directory and the alias working tree
/// are kept separate. This unique feature allows us to treat an entire
/// directory as a Git repository without needing to initialize it as one.
///
/// This technique does not really have a standard name despite being a common
/// method to manage dotfile configurations through Git. Se we call it the
/// __bare-alias technique__. Hence, the term _bare-alias_ repository!
///
/// # See Also
///
/// 1. [ArchWiki - dotfiles](https://wiki.archlinux.org/title/Dotfiles#Tracking_dotfiles_directly_with_Git)
pub struct Cluster {
    repository: Repository,
    pub definition: ClusterDefinition,
    sparse_checkout: SparsityDrafter<InvertedGitignore>,
}

impl Cluster {
    /// Initialize a new cluster.
    ///
    /// Will initialize a new cluster at target path based an initial cluster
    /// definition. Once properly initialized, the initial cluster definition
    /// will be serialized into the work tree alias, staged into the index,
    /// and committed.
    ///
    /// # Errors
    ///
    /// - Will fail if cluster cannot be initialized as a bare-alias repository.
    /// - Will fail if cluster definition cannot be serialized.
    /// - Will fail if newly serialized cluster definition cannot be staged
    ///   and committed.
    #[instrument(skip(path, definition), level = "debug")]
    pub fn try_new_init(path: impl AsRef<Path>, definition: ClusterDefinition) -> Result<Self> {
        info!("initialize new cluster: {:?}", path.as_ref().display());
        let repository = Repository::init_bare(path.as_ref())?;

        let mut config = repository.config()?;
        // INVARIANT: Do not show untracked files.
        config.set_str("status.showUntrackedFiles", "no")?;
        // INVARIANT: Always enable sparse checkout.
        config.set_str("core.sparseCheckout", "true")?;
        // INVARIANT: Allow changes to work tree alias outside of sparsity rules.
        config.set_str("advice.updateSparsePath", "false")?;
        let matcher = InvertedGitignore::new();
        let sparse_checkout = SparsityDrafter::new(path.as_ref(), matcher)?;

        let cluster = Self {
            repository,
            definition,
            sparse_checkout,
        };

        let contents = toml::ser::to_string_pretty(&cluster.definition)?;
        info!(
            "stage and commit the following cluster definition:\n{}",
            contents
        );
        cluster.stage_and_commit("cluster.toml", contents, "chore: add cluster.toml")?;
        cluster.gitcall_non_interactive(["checkout"])?;

        Ok(cluster)
    }

    /// Open existing cluster.
    ///
    /// Opens existing cluster at specified path and loads cluster definition
    /// at the top-level.
    ///
    /// # Errors
    ///
    /// - Will fail if cluster cannot be opened.
    /// - Will fail if cluster definition cannot be extracted at the top-level.
    /// - Will fail if missing configuration settings cannot be set.
    #[instrument(skip(path), level = "debug")]
    pub fn try_new_open(path: impl AsRef<Path>) -> Result<Self> {
        debug!("open cluster: {:?}", path.as_ref().display());
        let repository = Repository::open(path.as_ref())?;
        let definition = extract_cluster_definition(&repository)?;
        let matcher = InvertedGitignore::new();
        let sparse_checkout = SparsityDrafter::new(path.as_ref(), matcher)?;
        let cluster = Self {
            repository,
            definition,
            sparse_checkout,
        };

        // INVARIANT: Do not show untracked files.
        let mut config = cluster.repository.config()?;
        if cluster.get_config_value(&config, "status.showUntrackedFiles")? != Some("no".into()) {
            config.set_str("status.showUntrackedFiles", "no")?;
        }

        // INVARIANT: Always enable sparse checkout.
        if cluster.get_config_value(&config, "core.sparseCheckout")? != Some("true".into()) {
            config.set_str("core.sparseCheckout", "true")?;
        }

        // INVARIANT: Allow changes to work tree alias outside of sparsity rules.
        if cluster.get_config_value(&config, "advice.updateSparsePath")? != Some("true".into()) {
            config.set_str("advice.updateSparsePath", "false")?;
        }

        Ok(cluster)
    }

    /// Clone cluster from remote to target path.
    ///
    /// Clones cluster through valid Git URL to a target path. Will use
    /// authentication by prompting for user credentials if needed. Once the
    /// cluster has been cloned, its cluster definition will be extracted at
    /// the top-level. A simple progress bar will be displayed showing how
    /// much of the cluster has been cloned.
    ///
    /// # Errors
    ///
    /// - Will fail if cluster cannot be cloned for whatever reason.
    /// - Will fail if cluster definition cannot be extracted from top-level.
    pub fn try_new_clone(
        url: impl AsRef<str>,
        path: impl AsRef<Path>,
        prompter: ProgressBarAuthenticator,
    ) -> Result<Self> {
        let authenticator = GitAuthenticator::default().set_prompter(prompter.clone());
        let config = Config::open_default()?;
        let style = ProgressStyle::with_template(
            "{elapsed_precise:.green}  {msg:<50}  [{wide_bar:.yellow/blue}]",
        )?
        .progress_chars("-Cco.");
        prompter.bar.set_style(style);
        prompter.bar.set_message(url.as_ref().to_string());
        prompter
            .bar
            .enable_steady_tick(std::time::Duration::from_millis(100));

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

        let mut config = repository.config()?;
        // INVARIANT: Do not show untracked files.
        config.set_str("status.showUntrackedFiles", "no")?;
        // INVARIANT: Always enable sparse checkout.
        config.set_str("core.sparseCheckout", "true")?;
        // INVARIANT: Allow changes to work tree alias outside of sparsity rules.
        config.set_str("advice.updateSparsePath", "false")?;

        let definition = extract_cluster_definition(&repository)?;
        let matcher = InvertedGitignore::new();
        let sparse_checkout = SparsityDrafter::new(path.as_ref(), matcher)?;

        Ok(Self {
            repository,
            definition,
            sparse_checkout,
        })
    }

    /// Stage and commit content directly into the cluster.
    ///
    /// Takes a path and string content to be directly staged and committed
    /// into the cluster with a helpful message. Always appends the new commit
    /// after the latest parent commit.
    ///
    /// # Errors
    ///
    /// - Will fail latest parent commit cannot be determined.
    /// - Will fail if input content cannot be staged into the index.
    /// - Will fail if staged content cannot be committed.
    pub fn stage_and_commit(
        &self,
        filename: impl AsRef<Path>,
        contents: impl AsRef<str>,
        message: impl AsRef<str>,
    ) -> Result<()> {
        let entry = IndexEntry {
            ctime: IndexTime::new(0, 0),
            mtime: IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            file_size: contents.as_ref().len() as u32,
            id: self.repository.blob(contents.as_ref().as_bytes())?,
            flags: 0,
            flags_extended: 0,
            path: filename
                .as_ref()
                .as_os_str()
                .to_string_lossy()
                .into_owned()
                .as_bytes()
                .to_vec(),
        };

        // INVARIANT: Always use new tree produced by index after staging new entry.
        let mut index = self.repository.index()?;
        index.add_frombuffer(&entry, contents.as_ref().as_bytes())?;
        let tree_oid = index.write_tree()?;
        let tree = self.repository.find_tree(tree_oid)?;

        // INVARIANT: Always determine latest parent commits to append to.
        let signature = self.repository.signature()?;
        let mut parents = Vec::new();
        if let Some(parent) = self
            .repository
            .head()
            .ok()
            .map(|head| head.target().unwrap())
        {
            parents.push(self.repository.find_commit(parent)?);
        }
        let parents = parents.iter().collect::<Vec<_>>();

        // INVARIANT: Commit to HEAD by appending to obtained parent commits.
        self.repository.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message.as_ref(),
            &tree,
            &parents,
        )?;

        Ok(())
    }

    /// Deploy file content from cluster based on a set of sparsity rules.
    ///
    /// Each rule is written into the sparse checkout configuration file
    /// inside cluster. Once written, a checkout is performed to apply the
    /// new rules. Any file that matches the newly applied rules will be
    /// deployed directly to the work tree alias.
    ///
    /// # Errors
    ///
    /// - Will fail if sparsity rules cannot be inserted into sparse checkout
    ///   configuration file.
    /// - Will fail if checkout cannot be performed to apply the new rules.
    #[instrument(skip(self, rules), level = "debug")]
    pub fn deploy_rules(&self, rules: impl IntoIterator<Item = impl Into<String>>) -> Result<()> {
        info!("deploy {:?}", self.repository.path().display());
        if self.is_empty() {
            warn!("cluster {:?} is empty", self.repository.path().display());
            return Ok(());
        }

        self.sparse_checkout.insert_rules(rules)?;
        let output = self.gitcall_non_interactive(["checkout"])?;
        info!("{output}");

        Ok(())
    }

    /// Deploy all tracked files to work tree alias.
    ///
    /// # Errors
    ///
    /// - Will fail if sparse checkout configuration file cannot be opened,
    ///   read, and written to for whatever reason.
    /// - Will fail if checkout files.
    #[instrument(skip(self), level = "debug")]
    pub fn deploy_all(&self) -> Result<()> {
        info!("deploy all of {:?}", self.repository.path().display());
        if self.is_empty() {
            warn!("cluster {:?} is empty", self.repository.path().display());
            return Ok(());
        }

        self.sparse_checkout.clear_rules()?;
        self.sparse_checkout.insert_rules(["/*"])?;
        let output = self.gitcall_non_interactive(["checkout"])?;
        info!("{output}");
        Ok(())
    }

    /// Deploy default sparsity rules of cluster.
    ///
    /// Uses the sparsity rules defined in the cluster's definition file. Will
    /// not override existing rules in sparse checkout configuration file.
    ///
    /// # Errors
    ///
    /// - Will fail if sparse checkout configuration file cannot be opened,
    ///   read, and written to for whatever reason.
    /// - Will fail if checkout files.
    #[instrument(skip(self), level = "debug")]
    pub fn deploy_default_rules(&self) -> Result<()> {
        info!(
            "deploy default sparsity rules of {:?}",
            self.repository.path().display()
        );
        if self.is_empty() {
            warn!("cluster {:?} is empty", self.repository.path().display());
            return Ok(());
        }

        if let Some(default) = &self.definition.settings.include {
            self.sparse_checkout.insert_rules(default)?;
            let output = self.gitcall_non_interactive(["checkout"])?;
            info!("{output}");
        } else {
            warn!("cluster has no default sparsity rules to use");
        }

        Ok(())
    }

    /// Undeploy file content from cluster based on a set of sparsity rules.
    ///
    /// Each rule will be matched by the rules inside the sparse checkout
    /// configuration file. Any rules that match will be removed. Once the
    /// sparse checkout configuration file is done being updated, a checkout
    /// will be performed to apply the changes to work tree alias. The rules
    /// that were removed will cause target files to be also removed from
    /// the work tree alias.
    ///
    /// # Errors
    ///
    /// - Will fail if matching rules cannot be removed from sparse checkout
    ///   configuration file.
    /// - Will fail if checkout cannot be performed to apply new changes.
    #[instrument(skip(self, rules), level = "debug")]
    pub fn undeploy_rules(&self, rules: impl IntoIterator<Item = impl Into<String>>) -> Result<()> {
        info!("undeploy {:?}", self.repository.path().display());
        if self.is_empty() {
            warn!("cluster {:?} is empty", self.repository.path().display());
            return Ok(());
        }

        self.sparse_checkout.remove_rules(rules)?;
        let output = self.gitcall_non_interactive(["checkout"])?;
        info!("{output}");
        Ok(())
    }

    /// Undeploy the entire work tree alias.
    ///
    /// Similar to undeploy_rules, but removes _all_ existing sparsity
    /// rules from sparse checkout configuration file. Once all rules have
    /// been removed, a checkout is performed to apply the changes made. Since
    /// there are no sparsity rules, any and all deployed file content will be
    /// undeployed from the work tree alias.
    ///
    /// A warning will be issued if the cluster is already undeployed, i.e.,
    /// there is nothing in its work tree alias.
    ///
    /// # Errors
    ///
    /// - Will fail if sparsity rule set cannot be cleared from sparse checkout
    ///   configuration file.
    /// - Will fail if checkout cannot be performed to apply new changes.
    #[instrument(skip(self), level = "debug")]
    pub fn undeploy_all(&self) -> Result<()> {
        if !self.is_deployed() {
            warn!(
                "cluster {:?} already undeployed in full",
                self.repository.path().display()
            );
            return Ok(());
        }

        self.sparse_checkout.clear_rules()?;
        let output = self.gitcall_non_interactive(["checkout"])?;
        info!("{output}");

        Ok(())
    }

    /// Undeploy default sparsity rules of cluster.
    ///
    /// Uses the sparsity rules defined in the cluster's definition file. Will
    /// not override existing rules in sparse checkout configuration file.
    ///
    /// # Errors
    ///
    /// - Will fail if sparse checkout configuration file cannot be opened,
    ///   read, and written to for whatever reason.
    /// - Will fail if checkout files.
    #[instrument(skip(self), level = "debug")]
    pub fn undeploy_default_rules(&self) -> Result<()> {
        info!(
            "undeploy default sparsity rules of {:?}",
            self.repository.path().display()
        );
        if self.is_empty() {
            warn!("cluster {:?} is empty", self.repository.path().display());
            return Ok(());
        }

        if let Some(default) = &self.definition.settings.include {
            self.sparse_checkout.remove_rules(default)?;
        } else {
            warn!("cluster does not have default sparsity rules to use");
        }

        let output = self.gitcall_non_interactive(["checkout"])?;
        info!("{output}");

        Ok(())
    }

    /// Check if cluster is deployed.
    ///
    /// Checks if cluster is deployed by seeing if any file content exists in
    /// its work tree alias.
    pub fn is_deployed(&self) -> bool {
        if self.is_empty() {
            return false;
        }

        let rules = match self.sparse_checkout.current_rules() {
            Ok(r) => r,
            Err(_) => return false,
        };

        if rules.is_empty() {
            return false;
        }

        let entries = match self.list_file_paths() {
            Ok(p) => p,
            Err(_) => return false,
        };

        for entry in entries {
            let full_path = self
                .definition
                .settings
                .work_tree_alias
                .as_path()
                .join(&entry);
            if full_path.exists() && self.does_path_match_sparse_rule(&rules, &full_path) {
                return true;
            }
        }

        false
    }

    /// Print out current sparsity rule set.
    ///
    /// # Errors
    ///
    /// - Will fail if sparse checkout configuration file cannot be opened and
    ///   read.
    #[instrument(skip(self), level = "debug")]
    pub fn show_deploy_rules(&self) -> Result<()> {
        let rules = self.sparse_checkout.current_rules()?;
        info!("current sparisty rules {rules:#?}");

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub fn show_tracked_files(&self) -> Result<()> {
        let files = self.list_file_paths()?;
        info!("currently tracked files {files:#?}");

        Ok(())
    }

    /// Check if cluster is empty.
    ///
    /// A cluster is considered empty if it has not commit history.
    pub fn is_empty(&self) -> bool {
        self.repository
            .head()
            .ok()
            .and_then(|head| head.target())
            .and_then(|oid| self.repository.find_commit(oid).ok())
            .is_none()
    }

    /// Path to cluster.
    ///
    /// Returns path to gitdir of cluster.
    pub fn path(&self) -> &Path {
        self.repository.path()
    }

    /// Issue Git command via non interactive system call.
    ///
    /// Takes a a set of arguments and passes them on to Git itself in a
    /// non interactive (non-blocking) fashion. Returns any output from
    /// standard out or standard error.
    ///
    /// # Errors
    ///
    /// Will fail if system call to Git fails, or Git itself fails.
    pub fn gitcall_non_interactive(
        &self,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<String> {
        syscall_non_interactive("git", self.expand_bin_args(args))
    }

    /// Issue Git command via interactive system call.
    ///
    /// Takes a set of arguments and passes them on to Git itself in an
    /// interactive (blocking) fashion. Calls to this method will allow
    /// Git to control the currently active process that caller is using.
    /// Control will not be given back until Git is done executing.
    ///
    /// # Errors
    ///
    /// Will fail if system call to Git fails, or Git itself fails.
    pub fn gitcall_interactive(
        &self,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<()> {
        let index_before = self.get_staged_paths()?;
        syscall_interactive("git", self.expand_bin_args(args))?;
        let index_after = self.get_staged_paths()?;

        // INVARIANT: Sync sparsity rules with index if and only if the index itself has changed.
        let newly_added: Vec<PathBuf> = index_after.difference(&index_before).cloned().collect();
        if !newly_added.is_empty() {
            self.sync_sparse_with_new_files(&newly_added)?;
        }

        Ok(())
    }

    fn get_staged_paths(&self) -> Result<HashSet<PathBuf>> {
        let index = self.repository.index()?;
        let mut paths = HashSet::new();

        for entry in index.iter() {
            if let Ok(path_str) = std::str::from_utf8(&entry.path) {
                paths.insert(PathBuf::from(path_str));
            }
        }

        Ok(paths)
    }

    #[instrument(skip(self, new_files), level = "debug")]
    fn sync_sparse_with_new_files(&self, new_files: &[PathBuf]) -> Result<()> {
        let current_rules = self.sparse_checkout.current_rules()?;
        let mut new_rules = Vec::new();

        for path in new_files {
            let full_path = self
                .definition
                .settings
                .work_tree_alias
                .as_path()
                .join(path);

            debug!(
                "checking if {} matches existing sparse rules",
                path.display()
            );

            if !self.does_path_match_sparse_rule(&current_rules, &full_path) {
                debug!("adding new sparse rule for {}", path.display());
                new_rules.push(path.display().to_string());
            } else {
                debug!("{} already covered by existing rules", path.display());
            }
        }

        if !new_rules.is_empty() {
            info!("adding {} new sparse rules", new_rules.len());
            self.sparse_checkout.insert_rules(&new_rules)?;
            syscall_non_interactive("git", self.expand_bin_args(["checkout"]))?;
        }

        Ok(())
    }

    fn does_path_match_sparse_rule(&self, rules: &[String], path: &Path) -> bool {
        let root = self.definition.settings.work_tree_alias.as_path();
        let relative_path = match path.strip_prefix(root) {
            Ok(p) => p,
            Err(_) => return false,
        };
        let mut builder = GitignoreBuilder::new(root);

        // INVARIANT: Invert gitignore syntax to match sparsity rule syntax.
        //   - Ignore everything by default.
        //   - Sparsity rule "!dir/" means gitignore must ignore directory and all children.
        //   - Sparsity rule "dir/" means gitignore must unignore directory and all children.
        //   - Sparsity rule "!file" means gitignore must ignore file.
        //   - Sparsity rule "file" means gitignore must unignore file.
        builder.add_line(None, "/*").unwrap();
        for rule in rules {
            let is_negated = rule.starts_with('!');
            let pattern = rule.trim_start_matches('!');
            let is_dir = pattern.ends_with('/');

            if is_negated {
                builder.add_line(None, pattern).unwrap();
                if is_dir {
                    builder.add_line(None, &format!("{}**", pattern)).unwrap();
                }
            } else {
                builder.add_line(None, &format!("!{}", pattern)).unwrap();
                if is_dir {
                    builder.add_line(None, &format!("!{}**", pattern)).unwrap();
                }
            }
        }

        let matcher = builder.build().unwrap();
        !matcher
            .matched_path_or_any_parents(relative_path, path.is_dir())
            .is_ignore()
    }

    fn expand_bin_args(
        &self,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Vec<OsString> {
        let gitdir = self.repository.path().to_string_lossy().into_owned().into();
        let path_args: Vec<OsString> = vec![
            "--git-dir".into(),
            gitdir,
            "--work-tree".into(),
            self.definition.settings.work_tree_alias.to_os_string(),
        ];

        let mut user_args = args.into_iter().map(Into::into).collect::<Vec<_>>();
        if self.should_add_sparse_flag(&user_args) {
            user_args.splice(1..1, ["--sparse".into()]);
        }

        let mut bin_args: Vec<OsString> = Vec::new();
        bin_args.extend(path_args);
        bin_args.extend(user_args);

        bin_args
    }

    fn should_add_sparse_flag(&self, args: &[OsString]) -> bool {
        if args.is_empty() {
            return false;
        }

        let subcommand = args[0].to_string_lossy();
        matches!(subcommand.as_ref(), "add" | "rm" | "mv")
    }

    fn get_config_value(&self, config: &git2::Config, key: &str) -> Result<Option<String>> {
        match config.get_entry(key) {
            Ok(entry) => Ok(entry.value().map(|v| v.to_string())),
            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(err) => Err(anyhow!(err)),
        }
    }

    // Thank you Eric at https://www.hydrogen18.com/blog/list-all-files-git-repo-pygit2.html.
    fn list_file_paths(&self) -> Result<Vec<PathBuf>> {
        let mut entries = Vec::new();
        let commit = self.repository.head()?.peel_to_commit()?;
        let tree = commit.tree()?;
        let mut trees_and_paths = VecDeque::new();
        trees_and_paths.push_front((tree, PathBuf::new()));

        // Use DFS to traverse index tree.
        while let Some((tree, path)) = trees_and_paths.pop_front() {
            for tree_entry in &tree {
                match tree_entry.kind() {
                    // INVARIANT: Hit a tree? Traverse it!
                    Some(ObjectType::Tree) => {
                        let next_tree = self.repository.find_tree(tree_entry.id())?;
                        let next_path = path.join(bytes_to_path(tree_entry.name_bytes()));
                        trees_and_paths.push_front((next_tree, next_path));
                    }
                    // INVARIANT: Hit a blob? Record our current path!
                    Some(ObjectType::Blob) => {
                        let full_path = path.join(bytes_to_path(tree_entry.name_bytes()));
                        entries.push(full_path);
                    }
                    _ => continue,
                }
            }
        }

        Ok(entries)
    }
}

impl fmt::Debug for Cluster {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "located at {:?}", self.path().display())?;
        writeln!(f, "definition {:#?}", self.definition)
    }
}

#[derive(Debug, Clone)]
pub struct ProgressBarAuthenticator {
    pub(crate) bar: ProgressBar,
}

impl ProgressBarAuthenticator {
    pub fn new(bar: ProgressBar) -> Self {
        Self { bar }
    }
}

impl Prompter for ProgressBarAuthenticator {
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

fn syscall_non_interactive(
    cmd: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> Result<String> {
    let output = Command::new(cmd.as_ref()).args(args).output()?;
    let stdout = String::from_utf8_lossy(output.stdout.as_slice()).into_owned();
    let stderr = String::from_utf8_lossy(output.stderr.as_slice()).into_owned();
    let mut message = String::new();

    if !stdout.is_empty() {
        message.push_str(format!("stdout: {stdout}").as_str());
    }

    if !stderr.is_empty() {
        message.push_str(format!("stderr: {stderr}").as_str());
    }

    if !output.status.success() {
        return Err(anyhow!("command {:?} failed:\n{message}", cmd.as_ref()));
    }

    // INVARIANT: Chomp trailing newlines.
    let message = message
        .strip_suffix("\r\n")
        .or(message.strip_suffix('\n'))
        .map(ToString::to_string)
        .unwrap_or(message);

    Ok(message)
}

fn syscall_interactive(
    cmd: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> Result<()> {
    let status = Command::new(cmd.as_ref()).args(args).spawn()?.wait()?;
    if !status.success() {
        return Err(anyhow!("command {:?} failed", cmd.as_ref()));
    }

    Ok(())
}

fn extract_cluster_definition(repository: &Repository) -> Result<ClusterDefinition> {
    let commit = repository.head()?.peel_to_commit()?;
    let tree = commit.tree()?;
    let blob = tree
        .get_name("cluster.toml")
        .map(|entry| entry.to_object(repository)?.peel_to_blob())
        .ok_or(anyhow!("cluster has no definition file"))??;
    let content = String::from_utf8_lossy(blob.content()).into_owned();
    content
        .parse()
        .with_context(|| anyhow!("failed to extract cluster"))
}

// Thanks from:
//
// https://github.com/rust-lang/git2-rs/blob/5bc3baa9694a94db2ca9cc256b5bce8a215f9013/
// src/util.rs#L85
#[cfg(unix)]
fn bytes_to_path(bytes: &[u8]) -> &Path {
    use std::os::unix::prelude::*;
    Path::new(OsStr::from_bytes(bytes))
}
#[cfg(windows)]
fn bytes_to_path(byts: &[u8]) -> PathBuf {
    use std::str;
    Path::new(str::from_utf8(bytes).unwrap())
}
