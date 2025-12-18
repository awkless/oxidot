// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

//! Cluster store management and manipulation.
//!
//! Oxidot groups clusters together into one place called the __cluster store__.
//! The cluster store houses all available clusters that the user can manipulate
//! locally.
//!
//! # Cluster Store Layout
//!
//! The cluster store can generally be placed anywhere on the user's
//! file system. However, the default location is `$XDG_DATA_HOME/oxidot-store`.
//! Each cluster is given a its own unique local name. The name of a cluster
//! in the cluster store is the name of the directory that contains the
//! cluster itself. Each cluster entry is always given a ".git" extension.
//! So, `$XDG_DATA_HOME/oxidot-store/shell.git` means that the cluster store
//! contains a cluster named "shell".
//!
//! Oxidot only evaluates the top-level of the cluster store. Thus, it is not
//! possible to nest clusters inside one another. The closest the user can get
//! to this is by listing a cluster as a dependency of another cluster via
//! the cluster definition file.

use crate::{
    cluster::{Cluster, ClusterAccess, Git2Cluster},
    config::{ClusterDefinition, ClusterDependency},
};

use futures::{stream, StreamExt};
use indicatif::{MultiProgress, ProgressBar};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};
use tracing::{info, instrument, warn};

/// Cluster store handler.
#[derive(Debug)]
pub struct Store {
    state: Arc<Mutex<StoreState>>,
}

impl Store {
    /// Construct new cluster store manager.
    ///
    /// Will treat target directory as a cluster store. All clusters within
    /// that path will be opened for management and manipulation.
    ///
    /// # Errors
    ///
    /// - Return [`Error::Glob`] if cluster entry paths cannot be
    ///   globbed.
    /// - Return [`Error::GlobPattern`] if glob pattern for cluster
    ///   entry is invalid.
    /// - Return [`Error::Cluster`] if any cluster entry
    ///   cannot be opened.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        mkdirp::mkdirp(path.as_ref())?;

        let store = Self {
            state: Arc::new(Mutex::new(StoreState::new(path.as_ref(), HashMap::new()))),
        };

        let mut state = store.lock_state();
        let pattern = state
            .store_path
            .join("*.git")
            .to_string_lossy()
            .into_owned();
        for dir in glob::glob(pattern.as_str())? {
            // INVARIANT: The name of a cluster is the directory name minus the .git extension.
            let path = dir?;
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            state.clusters.insert(name, Git2Cluster::try_open(path)?);
        }
        drop(state);

        // TODO: Add checks for valid cluster store structure at some point.
        Ok(store)
    }

    /// Initialize new cluster into store.
    ///
    /// Takes a cluster definition to initialize a new cluster with it inside
    /// of the cluster store.
    ///
    /// # Errors
    ///
    /// - Return [`Error::Cluster`] if cluster cannot be
    ///   initialized.
    pub fn init_cluster(
        &self,
        name: impl Into<String>,
        definition: ClusterDefinition,
    ) -> Result<()> {
        let mut state = self.lock_state();
        let name = name.into();
        let path = state.store_path.join(format!("{}.git", &name));
        state
            .clusters
            .insert(name, Git2Cluster::try_init(path, definition)?);

        Ok(())
    }

    /// Remove cluster from store.
    ///
    /// Undeploys the target cluster, and removes it from the cluster store in
    /// full.
    ///
    /// # Errors
    ///
    /// - Return [`Error::Cluster`] if cluster cannot be
    ///   initialized.
    /// - Return [`Error::ClusterNotFound`] if cluster does not
    ///   exist.
    /// - Return [`Error::Io`] if cluster could not be deleted
    ///   from cluster store.
    pub fn remove_cluster(&self, name: impl AsRef<str>) -> Result<Cluster> {
        let mut state = self.lock_state();
        let removed =
            state
                .clusters
                .remove(name.as_ref())
                .ok_or(Error::ClusterNotFound {
                    name: name.as_ref().to_string(),
                })?;
        removed.undeploy_all()?;

        Ok(removed)
    }

    /// Clone a cluster along with its dependencies.
    ///
    /// Clones target cluster, and performs dependency resolution by looking
    /// for missing dependencies and cloning them into the store as well. The
    /// dependency resolution is done concurrently.
    ///
    /// The current progress of dependency cloning is shown via interactive
    /// progress bars that can prompt the user for credentials if need be
    /// during the cloning process.
    ///
    /// # Errors
    ///
    /// - Return [`Error::Cluster`] if cluster cannot be
    ///   initialized.
    pub async fn clone_cluster(
        &self,
        name: impl Into<String>,
        url: impl Into<String>,
    ) -> Result<()> {
        let name = name.into();
        let url = url.into();
        let multi_bar = MultiProgress::new();
        let mut bars = Vec::new();

        let (store_path, unresolved) = {
            let mut state = self.lock_state();
            let path = state.store_path.join(format!("{}.git", &name));
            let bar = multi_bar.add(ProgressBar::no_length());
            bars.push(bar.clone());

            let cluster = Git2Cluster::try_clone(&url, &path, bar)?;
            let unresolved = state.find_unresolved_dependencies(&cluster.definition)?;
            state.clusters.insert(name.clone(), cluster);

            (state.store_path.clone(), unresolved)
        };

        let results = Arc::new(Mutex::new(Vec::new()));
        stream::iter(unresolved)
            .for_each_concurrent(None, |dep| {
                let results = results.clone();
                let store_path = store_path.clone();
                let bar = multi_bar.add(ProgressBar::no_length());
                bars.push(bar.clone());

                async move {
                    let result = tokio::spawn(async move {
                        let path = store_path.join(format!("{}.git", &dep.name));
                        let cluster = Git2Cluster::try_clone(&dep.url, &path, bar.clone())?;
                        bar.finish();
                        Ok::<_, Error>((dep.name.clone(), cluster))
                    })
                    .await;
                    let mut results = results.lock().unwrap();
                    results.push(result);
                    drop(results);
                }
            })
            .await;

        for bar in bars {
            bar.finish();
        }

        let results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
        let clusters = results
            .into_iter()
            .flatten()
            .collect::<Result<Vec<_>, _>>()?;

        let mut state = self.lock_state();
        for (name, cluster) in clusters {
            state.clusters.insert(name, cluster);
        }

        Ok(())
    }

    /// Use target cluster for stuff.
    ///
    /// Finds target cluster in store, and uses the clouser to perform some
    /// kind of action with it.
    ///
    /// # Errors
    ///
    /// - Return [`Error::ClusterNotFound`] if cluster does not
    ///   exist.
    /// - Fails if clouser also fails for whatever reason.
    pub fn use_cluster<C, R>(&self, name: impl AsRef<str>, usage: C) -> Result<R>
    where
        C: FnOnce(&Cluster) -> Result<R>,
    {
        let state = self.lock_state();
        let cluster =
            state
                .clusters
                .get(name.as_ref())
                .ok_or(Error::ClusterNotFound {
                    name: name.as_ref().into(),
                })?;

        usage(cluster)
    }

    /// Give detailed status information about cluster store.
    ///
    /// Prints the following information:
    ///
    /// - Deployment status.
    /// - Cluster name.
    /// - Cluster work tree alias
    /// - Cluster description.
    /// - Cluster remote URL.
    /// - Default deployment rules.
    /// - Dependencies of the cluster.
    #[instrument(skip(self), level = "debug")]
    pub fn detailed_status(&self) {
        let state = self.lock_state();
        if state.clusters.is_empty() {
            warn!("cluster store is empty");
            return;
        }

        let mut status = String::new();
        for (name, entry) in state.clusters.iter() {
            let deployment = if entry.is_deployed() {
                "[  deployed]"
            } else {
                "[undeployed]"
            };

            let data = format!(
                "{} {} -> {} : {}\n  url: {}\n  include: {:#?}\n  dependencies: {:#?}\n",
                deployment,
                name,
                entry.definition.settings.work_tree_alias,
                entry.definition.settings.description,
                entry.definition.settings.url,
                entry.definition.settings.include,
                entry.definition.dependencies,
            );
            status.push_str(data.as_str());
        }

        info!("all avaliable clusters:\n{}", status);
    }

    /// Give status information for deployed clusters only.
    ///
    /// Prints the following information:
    ///
    /// - Deployment status.
    /// - Cluster name.
    /// - Cluster work tree alias
    /// - Cluster description.
    /// - Cluster remote URL.
    /// - Default deployment rules.
    /// - Dependencies of the cluster.
    #[instrument(skip(self), level = "debug")]
    pub fn deployed_only_status(&self) {
        let state = self.lock_state();
        if state.clusters.is_empty() {
            warn!("cluster store is empty");
            return;
        }

        let mut status = String::new();
        for (name, entry) in state.clusters.iter() {
            if entry.is_deployed() {
                let data = format!(
                    "{} -> {} : {}\n  url: {}\n  include: {:#?}\n  dependencies: {:#?}\n",
                    name,
                    entry.definition.settings.work_tree_alias,
                    entry.definition.settings.description,
                    entry.definition.settings.url,
                    entry.definition.settings.include,
                    entry.definition.dependencies,
                );
                status.push_str(data.as_str());
            }
        }

        info!("all deployed clusters:\n{}", status);
    }

    /// Give status information of undeployed clusters only.
    ///
    /// Prints the following information:
    ///
    /// - Deployment status.
    /// - Cluster name.
    /// - Cluster work tree alias
    /// - Cluster description.
    /// - Cluster remote URL.
    /// - Default deployment rules.
    /// - Dependencies of the cluster.
    #[instrument(skip(self), level = "debug")]
    pub fn undeployed_only_status(&self) {
        let state = self.lock_state();
        if state.clusters.is_empty() {
            warn!("cluster store is empty");
            return;
        }

        let mut status = String::new();
        for (name, entry) in state.clusters.iter() {
            if !entry.is_deployed() {
                let data = format!(
                    "{} -> {} : {}\n  url: {}\n  include: {:#?}\n  dependencies: {:#?}\n",
                    name,
                    entry.definition.settings.work_tree_alias,
                    entry.definition.settings.description,
                    entry.definition.settings.url,
                    entry.definition.settings.include,
                    entry.definition.dependencies,
                );
                status.push_str(data.as_str());
            }
        }

        info!("all deployed clusters:\n{}", status);
    }

    /// Show current deployment rules for target cluster.
    ///
    /// # Errors
    ///
    /// - Return [`Error::Cluster`] if sparse checkout configuration
    ///   file could not be opened to get rule set.
    #[instrument(skip(self, name), level = "debug")]
    pub fn deploy_rules_status(&self, name: impl AsRef<str>) -> Result<()> {
        self.use_cluster(name.as_ref(), |cluster| {
            let rule_set = cluster.list_deploy_rules()?;
            info!(
                "current deploy rules for {}:\n  {:#?}",
                name.as_ref(),
                rule_set
            );

            Ok(())
        })?;

        Ok(())
    }

    /// Give listing of currently tracked files for target cluster.
    ///
    /// # Errors
    ///
    /// - Return [`Error::Cluster`] if paths could not be obtained
    ///   from cluster's index.
    #[instrument(skip(self, name), level = "debug")]
    pub fn tracked_files_status(&self, name: impl AsRef<str>) -> Result<()> {
        self.use_cluster(name.as_ref(), |cluster| {
            let files = cluster.list_tracked_files()?;
            info!(
                "current tracked files for {}:\n {:#?}",
                name.as_ref(),
                files
            );

            Ok(())
        })?;

        Ok(())
    }

    #[inline]
    fn lock_state(&self) -> MutexGuard<'_, StoreState> {
        self.state.lock().unwrap()
    }
}

#[derive(Debug)]
pub(crate) struct StoreState {
    pub(crate) store_path: PathBuf,
    pub(crate) clusters: HashMap<String, Cluster>,
}

impl StoreState {
    pub(crate) fn new(store_path: impl Into<PathBuf>, clusters: HashMap<String, Cluster>) -> Self {
        Self {
            store_path: store_path.into(),
            clusters,
        }
    }

    pub(crate) fn find_unresolved_dependencies(
        &self,
        parent: &ClusterDefinition,
    ) -> Result<Vec<ClusterDependency>> {
        let mut unresolved = Vec::new();
        let mut visited = HashSet::new();
        let mut stack = VecDeque::new();

        if let Some(dependencies) = &parent.dependencies {
            if !dependencies.is_empty() {
                stack.push_back(dependencies[0].clone());
            }
        } else {
            return Ok(Vec::new());
        }

        while let Some(current) = stack.pop_back() {
            if !visited.insert(current.clone()) {
                continue;
            }

            if !self.clusters.contains_key(&current.name) {
                unresolved.push(current.clone());
            }

            if let Some(cluster) = self.clusters.get(&current.name) {
                if let Some(deps) = &cluster.definition.dependencies {
                    for dep in deps {
                        stack.push_back(dep.clone());
                    }
                }
            }
        }

        Ok(unresolved)
    }
}

/// All possible error types for cluster store interaction.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Cluster does not existing in cluster store for some reason.
    #[error("cluster {name:?} not found in cluster store")]
    ClusterNotFound { name: String },

    /// Failed to use glob patterns for directory processing.
    #[error(transparent)]
    Glob(#[from] glob::GlobError),

    /// Glob pattern was not parsed correctly for some reason.
    #[error(transparent)]
    GlobPattern(#[from] glob::PatternError),

    /// Cluster domain and deployment logic fails for some reason.
    #[error(transparent)]
    Cluster(#[from] crate::cluster::Error),

    /// Threads failed to properly join to main thread.
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),

    /// Input/Output operations failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Friendly result alias :3
type Result<T, E = Error> = std::result::Result<T, E>;
