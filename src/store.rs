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
    config::ClusterDefinition,
};

use futures::future::join_all;
use indicatif::{MultiProgress, ProgressBar};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};
use tracing::{info, instrument, warn};

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

    pub(crate) fn find_unresolved_dependencies(&self, parent: &str) -> Result<Vec<String>> {
        let mut unresolved = Vec::new();
        let mut visited = HashSet::new();
        let mut stack = VecDeque::new();
        stack.push_back(parent.to_string());

        while let Some(current) = stack.pop_back() {
            if !visited.insert(current.clone()) {
                continue;
            }

            if !self.clusters.contains_key(&current) {
                unresolved.push(current.clone());
            }

            if let Some(cluster) = self.clusters.get(&current) {
                if let Some(deps) = &cluster.definition.dependencies {
                    for dep in deps {
                        stack.push_back(dep.name.clone());
                    }
                }
            }
        }

        Ok(unresolved)
    }
}

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
    /// - Return [`ClusterStoreError::Glob`] if cluster entry paths cannot be
    ///   globbed.
    /// - Return [`ClusterStoreError::GlobPattern`] if glob pattern for cluster
    ///   entry is invalid.
    /// - Return [`ClusterStoreError::ClusterError`] if any cluster entry
    ///   cannot be opened.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
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

    pub fn init_cluster(
        &self,
        name: impl Into<String>,
        definition: ClusterDefinition,
    ) -> Result<()> {
        let mut state = self.lock_state();
        let name = name.into();
        let path = state.store_path.join(&name).join(".git");
        state
            .clusters
            .insert(name, Git2Cluster::try_init(path, definition)?);

        Ok(())
    }

    pub fn remove_cluster(&self, name: impl AsRef<str>) -> Result<Cluster> {
        let mut state = self.lock_state();
        let removed =
            state
                .clusters
                .remove(name.as_ref())
                .ok_or(ClusterStoreError::ClusterNotFound {
                    name: name.as_ref().to_string(),
                })?;
        removed.undeploy_all()?;

        Ok(removed)
    }

    pub async fn clone_cluster(
        &self,
        name: impl Into<String>,
        url: impl Into<String>,
    ) -> Result<()> {
        let name = name.into();
        let url = url.into();

        let (store_path, unresolved) = {
            let mut state = self.lock_state();

            let path = state.store_path.join(&name).join(".git");
            let bars = MultiProgress::new();
            let bar = bars.add(ProgressBar::no_length());

            let cluster = Git2Cluster::try_clone(&url, &path, bar.clone())?;
            bar.finish();
            state.clusters.insert(name.clone(), cluster);

            let unresolved = state.find_unresolved_dependencies(&name)?;
            (state.store_path.clone(), unresolved)
        };

        let tasks = unresolved.into_iter().map(|dep| {
            let store_path = store_path.clone();
            let url = url.clone();
            tokio::task::spawn_blocking(move || {
                let path = store_path.join(&dep).join(".git");
                let bar = ProgressBar::no_length();
                let cluster = Git2Cluster::try_clone(&url, &path, bar)?;
                Ok::<_, ClusterStoreError>((dep, cluster))
            })
        });

        let results = join_all(tasks).await;
        let mut state = self.lock_state();
        for result in results {
            let (name, cluster) = result??;
            state.clusters.insert(name, cluster);
        }

        Ok(())
    }

    pub fn use_cluster<C, R>(&self, name: impl AsRef<str>, usage: C) -> Result<R>
    where
        C: FnOnce(&Cluster) -> Result<R>,
    {
        let state = self.lock_state();
        let cluster =
            state
                .clusters
                .get(name.as_ref())
                .ok_or(ClusterStoreError::ClusterNotFound {
                    name: name.as_ref().into(),
                })?;

        usage(cluster)
    }

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

    #[inline]
    fn lock_state(&self) -> MutexGuard<'_, StoreState> {
        self.state.lock().unwrap()
    }
}

/// All possible error types for cluster store interaction.
#[derive(Debug, thiserror::Error)]
pub enum ClusterStoreError {
    #[error("cluster {name:?} not found in cluster store")]
    ClusterNotFound { name: String },

    #[error(transparent)]
    Glob(#[from] glob::GlobError),

    #[error(transparent)]
    GlobPattern(#[from] glob::PatternError),

    #[error(transparent)]
    Cluster(#[from] crate::cluster::ClusterError),

    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}

/// Friendly result alias :3
type Result<T, E = ClusterStoreError> = std::result::Result<T, E>;
