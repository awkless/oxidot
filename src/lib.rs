// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-FileCopyrightText: 2024-2025 Eric Urban <hydrogen18@gmail.com>
// SPDX-License-Identifier: MIT

use anyhow::{anyhow, Context, Result};
use auth_git2::{GitAuthenticator, Prompter};
use git2::{
    build::RepoBuilder, Config, FetchOptions, IndexEntry, IndexTime, ObjectType, RemoteCallbacks,
    Repository,
};
use ignore::gitignore::GitignoreBuilder;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use inquire::{Password, Text};
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::Iter,
    collections::{HashMap, HashSet, VecDeque},
    ffi::{OsStr, OsString},
    fmt,
    fmt::Write,
    fs::{read_to_string, remove_dir_all, write, OpenOptions},
    path::{Path, PathBuf},
    process::Command,
    time,
};
use tracing::{info, instrument, warn};

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
pub struct Store {
    store_path: PathBuf,
    clusters: HashMap<String, Cluster>,
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
            clusters,
        };
        Ok(store)
    }

    /// Insert a cluster into the store.
    ///
    /// Returns `None` if and only if the cluster was new to the store. If the
    /// given cluster already exists in the store, then the old cluster will be
    /// returned after the new cluster takes its place in the store. The name
    /// of the cluster is never updated.
    pub fn insert(&mut self, name: impl Into<String>, cluster: Cluster) -> Option<Cluster> {
        self.clusters.insert(name.into(), cluster)
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
    pub fn remove(&mut self, name: impl AsRef<str>) -> Result<Cluster> {
        info!("remove {:?} from cluster store", name.as_ref());
        let cluster_path = self.store_path.join(format!("{}.git", name.as_ref()));
        let cluster = self.clusters
            .remove(name.as_ref())
            .ok_or(anyhow!("cluster {:?} not in store", name.as_ref()))?;
        cluster.undeploy_all()?;

        remove_dir_all(&cluster_path)
            .with_context(|| anyhow!("failed to remove {:?} from store", name.as_ref()))?;

        Ok(cluster)
    }

    /// Get cluster from the store.
    ///
    /// # Errors
    ///
    /// Will fail if cluster does not exist in the store.
    pub fn get(&self, name: impl AsRef<str>) -> Result<&Cluster> {
        self.clusters
            .get(name.as_ref())
            .ok_or(anyhow!("cluster {:?} not in store", name.as_ref()))
    }

    /// Get mutable cluster from the store.
    ///
    /// # Errors
    ///
    /// Will fail if cluster does not exist in the store.
    pub fn get_mut(&mut self, name: impl AsRef<str>) -> Result<&mut Cluster> {
        self.clusters
            .get_mut(name.as_ref())
            .ok_or(anyhow!("cluster {:?} not in store", name.as_ref()))
    }

    /// List all available clusters with full details.
    #[instrument(skip(self), level = "debug")]
    pub fn list_fully(&self) {
        if self.clusters.is_empty() {
            warn!("cluster store is empty");
            return;
        }

        let mut listing = String::new();
        for (name, entry) in self.clusters.iter() {
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
        if self.clusters.is_empty() {
            warn!("cluster store is empty");
            return;
        }

        let mut listing = String::new();
        for (name, entry) in self.clusters.iter() {
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
        if self.clusters.is_empty() {
            warn!("cluster store is empty");
            return;
        }

        let mut listing = String::new();
        for (name, entry) in self.clusters.iter() {
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

    /// Iterate through cluster store entries.
    pub fn iter(&self) -> Iter<'_, String, Cluster> {
        self.clusters.iter()
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
    pub fn resolve_dependencies(&mut self, cluster_name: impl AsRef<str>) -> Result<Vec<String>> {
        let bars = MultiProgress::new();
        let mut resolved = Vec::new();
        let mut visited = HashSet::new();
        let mut stack = VecDeque::new();
        stack.push_back(cluster_name.as_ref().to_string());

        while let Some(current) = stack.pop_back() {
            if !visited.insert(current.clone()) {
                continue;
            }

            if !self.clusters.contains_key(&current) {
                let bar = bars.add(ProgressBar::no_length());
                self.clone_missing_cluster(bar.clone(), &current)?;
                bar.finish();
            }

            if let Some(cluster) = self.clusters.get(&current) {
                resolved.push(current.clone());
                if let Some(deps) = &cluster.definition.dependencies {
                    for dep in deps {
                        if !visited.contains(&dep.name) {
                            stack.push_back(dep.name.clone());
                        }
                    }
                }
            }
        }

        Ok(resolved)
    }

    fn clone_missing_cluster(&mut self, bar: ProgressBar, name: impl AsRef<str>) -> Result<()> {
        let dep_info = self
            .clusters
            .values()
            .flat_map(|c| c.definition.dependencies.iter().flatten())
            .find(|d| d.name == name.as_ref())
            .ok_or_else(|| anyhow!("dependency {:?} no declared", name.as_ref()))?;

        let path = self.store_path.join(format!("{}.git", name.as_ref()));
        let auth_bar = ProgressBarAuthenticator::new(bar.clone());
        let cluster = Cluster::try_new_clone(&dep_info.url, path, auth_bar)?;
        self.clusters.insert(name.as_ref().into(), cluster);

        Ok(())
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
    sparse_checkout: SparseCheckout,
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
        let sparse_checkout = SparseCheckout::new(path.as_ref())?;

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
        info!("open cluster: {:?}", path.as_ref().display());
        let repository = Repository::open(path.as_ref())?;
        let mut cluster = Self {
            repository,
            definition: ClusterDefinition::default(),
            sparse_checkout: SparseCheckout::new(path.as_ref())?,
        };
        cluster.extract_cluster_definition()?;

        // INVARIANT: Do not show untracked files.
        let mut config = cluster.repository.config()?;
        if cluster.get_config_value(&config, "status.showUntrackedFiles")? != Some("no".into()) {
            config.set_str("status.showUntrackedFiles", "no")?;
        }

        // INVARIANT: Always enable sparse checkout.
        if cluster.get_config_value(&config, "core.sparseCheckout")? != Some("true".into()) {
            config.set_str("core.sparseCheckout", "true")?;
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

        let mut cluster = Self {
            repository,
            definition: ClusterDefinition::default(),
            sparse_checkout: SparseCheckout::new(path.as_ref())?,
        };
        cluster.extract_cluster_definition()?;
        cluster.deploy_default_rules()?;

        Ok(cluster)
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
        } else {
            warn!("cluster has no default sparsity rules to use");
        }

        let output = self.gitcall_non_interactive(["checkout"])?;
        info!("{output}");

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
        syscall_interactive("git", self.expand_bin_args(args))
    }

    fn extract_cluster_definition(&mut self) -> Result<()> {
        let commit = self.repository.head()?.peel_to_commit()?;
        let tree = commit.tree()?;
        let blob = tree
            .get_name("cluster.toml")
            .map(|entry| entry.to_object(&self.repository)?.peel_to_blob())
            .ok_or(anyhow!("cluster has no definition file"))??;
        let content = String::from_utf8_lossy(blob.content()).into_owned();
        self.definition = toml::de::from_str::<ClusterDefinition>(&content)?;

        Ok(())
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

        let mut bin_args: Vec<OsString> = Vec::new();
        bin_args.extend(path_args);
        bin_args.extend(args.into_iter().map(Into::into));

        bin_args
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

        while let Some((tree, path)) = trees_and_paths.pop_front() {
            for tree_entry in &tree {
                match tree_entry.kind() {
                    Some(ObjectType::Tree) => {
                        let next_tree = self.repository.find_tree(tree_entry.id())?;
                        let next_path = path.join(bytes_to_path(tree_entry.name_bytes()));
                        trees_and_paths.push_front((next_tree, next_path));
                    }
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

    fn does_path_match_sparse_rule(&self, rules: &[String], path: &Path) -> bool {
        let root = self.definition.settings.work_tree_alias.as_path();
        let relative_path = match path.strip_prefix(root) {
            Ok(p) => p,
            Err(_) => return false,
        };

        let mut builder = GitignoreBuilder::new(root);
        // INVARIANT: Ignore everything by default.
        builder.add_line(None, "/*").unwrap();

        for rule in rules {
            // INVARIANT: Negate/invert sparsity rules.
            // - Sparse checkout rules use the same syntax as gitignore rules, but include
            //   matching paths instead of ignoring them. So, we need to negate/invert these rules,
            //   because the ignore crate API ties to ignore matching paths instead of including
            //   them!
            let whitelist_rule = if let Some(stripped) = rule.strip_prefix('!') {
                stripped.to_string()
            } else {
                format!("!{}", rule)
            };
            builder.add_line(None, &whitelist_rule).unwrap();
        }

        let matcher = builder.build().unwrap();
        !matcher.matched(relative_path, path.is_dir()).is_ignore()
    }
}

/// Cluster definition layout.
///
/// All clusters in oxidot come with a __definition__ file. This file is a
/// simple configuration file that details how the cluster should be
/// configured and managed by not only the cluster itself, but by the cluster
/// store manager as well.
///
/// # General Layout
///
/// A cluster definition is composed of two basic parts: settings and
/// dependencies. The settings section simply defines how the cluster should
/// be configured. The dependencies section lists all dependencies that should
/// be deployed along with the cluster itself. In other words, clusters can
/// list other clusters as dependencies.
///
/// # Location
///
/// Cluster definitions must always exist at the top-level of a cluster in a
/// special file named "cluster.toml". If this file cannot be found, then the
/// cluster is invalid.
///
/// # See Also
///
/// - [`Cluster`](struct.Cluster)
/// - [`Store`](struct.Store)
#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ClusterDefinition {
    pub settings: ClusterSettings,
    pub dependencies: Option<Vec<ClusterDependency>>,
}

/// Cluster configuration settings.
///
/// Standard settings to use for any given cluster.
#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ClusterSettings {
    /// Brief description of what the cluster contains.
    pub description: String,

    /// Remove URL to clone cluster from.
    pub url: String,

    /// Work tree alias to use for deployment.
    pub work_tree_alias: WorkTreeAlias,

    /// Default listing of file content to deploy to work tree alias.
    pub include: Option<Vec<String>>,
}

/// Cluster dependency listing.
///
/// List of other clusters to use as dependencies for given cluster.
#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ClusterDependency {
    /// Name of the cluster dependency.
    pub name: String,

    /// Remote URL to clone cluster from if it isn't in the cluster store.
    pub url: String,

    /// Additional listing of file content to deploy.
    pub include: Option<Vec<String>>,
}

/// Work tree alias path for cluster.
///
/// # See also
///
/// - [`Cluster`](struct.Cluster)
#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct WorkTreeAlias(pub PathBuf);

impl WorkTreeAlias {
    /// Construct new work tree alias path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// Construct work tree alias path pointing to user's home directory.
    ///
    /// The user's home directory is the default path of a work tree alias
    /// if no other path is specified.
    ///
    /// # Errors
    ///
    /// Will fail if user's home directory cannot be determined for whatever
    /// reason.
    pub fn try_default() -> Result<Self> {
        Ok(Self(home_dir()?))
    }

    /// Helper to convert work tree alias path to [`OsString`].
    pub fn to_os_string(&self) -> OsString {
        OsString::from(self.0.to_string_lossy().into_owned())
    }

    /// Treat as path slice.
    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }
}

impl fmt::Display for WorkTreeAlias {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.write_str(self.0.display().to_string().as_str())
    }
}

/// Sparse checkout configuration file manager.
///
/// Manage the sparsity rules contained inside of a cluster's sparse checkout
/// configuration file.
///
/// # Why Sparse Checkout?
///
/// Git comes with a cool feature called "sparse checkout". It allows the user
/// to reduce their work tree to a subset of tracked files. What gets included
/// in this reduced work tree is determined by a set of __sparsity rules__.
/// A sparsity rule is just a pattern of characters that match tracked files
/// for inclusion into the reduced work tree. The formatting of these rules
/// use the same format as gitignore rules, but instead of trying to _ignore_
/// files, they are trying to _include_ them.
///
/// Sparse checkout operates in one of two modes: cone or non-cone mode. These
/// operation modes simply reduce the allowable set of sparsity rule patterns
/// that can be used. Cone mode only allows for usage of sparsity rule patterns
/// that include directories. Non-cone mode allows usage of the _entire_
/// sparsity rule pattern set. By default Git uses cone mode.
///
/// Oxidot employs sparse checkout as the backbone of the file deployment
/// feature provided by clusters. When using sparse checkout with bare-alias
/// repositories, file content can be directly deployed to a work tree alias
/// without needing to manually symlink, copy, or move it. This also has the
/// added benefit of allowing Git itself to keep track of these deployed files
/// without needing to modify the commit history of any given cluster in the
/// cluster store.
///
/// # Pitfalls
///
/// By default oxidot uses cone mode for sparse checkout. We prefer to give
/// the user full access to the sparsity rule pattern set to make it easier
/// to deploy any component of a cluster. However, cone mode is deprecated for
/// new releases of Git. Cone mode will not be removed from Git, but the
/// implementors of sparse checkout highly recommend to use non-cone mode
/// instead.
///
/// The main problem with cone mode is its runtime of O(N*M) where N is number
/// of sparsity rules to check, and M is the number of paths to check against.
/// Oxidot generally expects to avoid this issue through the modular setup
/// of clusters, where the runtime is split across multiple bare-alias
/// repositories.
///
/// # See Also
///
/// - [Man page sparse checkout](https://git-scm.com/docs/git-sparse-checkout)
/// - [`Cluster`](struct.Cluster)
#[derive(Debug)]
pub struct SparseCheckout {
    sparse_path: PathBuf,
}

impl SparseCheckout {
    /// Construct new sparse checkout configuration file manager.
    ///
    /// Determines path to sparse checkout configuration file relative to
    /// path to gitdir. Creates the sparse checkout configuration file if it
    /// does not already exist.
    ///
    /// # Errors
    ///
    /// Will fail if sparse checkout configuration file cannot be created.
    pub fn new(gitdir: impl Into<PathBuf>) -> Result<Self> {
        let sparse_path = gitdir.into().join("info").join("sparse-checkout");

        // INVARIANT: Create sparse checkout file if needed.
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&sparse_path)
            .with_context(|| anyhow!("failed to create {:?}", sparse_path.display()))?;

        Ok(Self { sparse_path })
    }

    /// Insert a list of sparsity rules to configuration file.
    ///
    /// Takes list of new sparsity rules and writes them directly into the
    /// sparse checkout configuration file. Any existing sparsity rules within
    /// the configuration file will be preserved, i.e., the new rules will be
    /// appended to the older rules. If there are duplicate sparsity rules, they
    /// will be deduplicated ensuring that each rule in the configuration is
    /// unique.
    ///
    /// # Errors
    ///
    /// Will fail if sparse checkout configuration file cannot be opened, read,
    /// or written to for whatever reason.
    pub fn insert_rules(&self, rules: impl IntoIterator<Item = impl Into<String>>) -> Result<()> {
        let mut rule_set = read_to_string(&self.sparse_path)
            .with_context(|| anyhow!("failed to read {:?}", self.sparse_path.display()))?;

        // INVARIANT: Append new rules to existing rule set.
        let new_rules: Vec<String> = rules.into_iter().map(|r| r.into()).collect();
        for rule in new_rules {
            writeln!(&mut rule_set, "{}", rule).unwrap();
        }

        // INVARIANT: Deduplicate rule set and make sure each rule is on its own line.
        let mut seen = HashSet::new();
        let rule_set = rule_set
            .lines()
            .filter(|line| seen.insert(*line))
            .collect::<Vec<_>>()
            .join("\n");

        // INVARIANT: Append trailing newline to end of rule set.
        let rule_set = if rule_set.is_empty() {
            rule_set
        } else {
            format!("{}\n", rule_set)
        };

        write(&self.sparse_path, rule_set.as_bytes()).with_context(|| {
            anyhow!(
                "failed to write sparsity rules to {:?}",
                self.sparse_path.display()
            )
        })?;

        Ok(())
    }

    /// Remove matching rules from sparse checkout configuration file.
    ///
    /// Takes list of rules and removes them from the sparse checkout
    /// configuration file if they exist. Any existing rules that do not match
    /// the list will be preserved.
    ///
    /// # Errors
    ///
    /// Will fail if sparse checkout configuration file cannot be opened, read,
    /// or written to for whatever reason.
    pub fn remove_rules(&self, rules: impl IntoIterator<Item = impl Into<String>>) -> Result<()> {
        let content = read_to_string(&self.sparse_path)
            .with_context(|| anyhow!("failed to read {:?}", self.sparse_path.display()))?;

        let old_rules: HashSet<String> = rules.into_iter().map(|rule| rule.into()).collect();
        let filtered: Vec<&str> = content
            .lines()
            .filter(|line| !old_rules.contains(*line))
            .collect();

        // INVARIANT: Append trailing newline to end of rule set.
        let result = if filtered.is_empty() {
            String::new()
        } else {
            format!("{}\n", filtered.join("\n"))
        };

        write(&self.sparse_path, result.as_bytes()).with_context(|| {
            anyhow!("failed to remove rules in {:?}", self.sparse_path.display())
        })?;

        Ok(())
    }

    /// Get listing of all current sparsity rules.
    ///
    /// # Errors
    ///
    /// Will fail if sparse checkout configuration file cannot be opened or
    /// read.
    pub fn current_rules(&self) -> Result<Vec<String>> {
        let content = read_to_string(&self.sparse_path)
            .with_context(|| anyhow!("failed to read {:?}", self.sparse_path.display()))?;

        Ok(content.lines().map(String::from).collect())
    }

    /// Clear all rules from sparse checkout configuration file.
    ///
    /// # Errors
    ///
    /// Will fail if sparse checkout configuration file cannot be opened, read,
    /// or written to for whatever reason.
    pub fn clear_rules(&self) -> Result<()> {
        write(&self.sparse_path, b"").with_context(|| {
            anyhow!("failed to clear rules in {:?}", self.sparse_path.display())
        })?;

        Ok(())
    }
}

#[derive(Clone)]
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

/// Determine path to user's home directory.
///
/// User's home directory acts as the default path for work tree aliases if
/// no other path is specified.
///
/// # Errors
///
/// Will fail if user's home directory cannot be determined for whatever
/// reason.
pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or(anyhow!("cannot determine path to home directory"))
}

/// Determine path to cluster store directory.
///
/// The cluster store path is set to `$XDG_DATA_HOME/oxidot-store` by default.
///
/// # Errors
///
/// Will fail if user's home directory cannot be determined for whatever
/// reason.
pub fn cluster_store_dir() -> Result<PathBuf> {
    dirs::data_dir()
        .map(|path| path.join("oxidot-store"))
        .ok_or(anyhow!("cannot determine path to cluster store"))
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
