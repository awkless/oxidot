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

use crate::cluster::domain::{Cluster, Git2Cluster, ClusterAccess};

use std::{collections::HashMap, path::{PathBuf, Path}};

pub struct ClusterStore {
    store_path: PathBuf,
    clusters: HashMap<String, Cluster>,
}

impl ClusterStore {
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
        let store_path = path.as_ref().to_path_buf();
        let pattern = store_path.join("*.git").to_string_lossy().into_owned();
        let mut clusters = HashMap::new();
        for entry in glob::glob(pattern.as_str())? {
            // INVARIANT: The name of a cluster is the directory name minus the .git extension.
            let path = entry?;
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            clusters.insert(name, Git2Cluster::try_open(path)?);
        }

        // TODO: Add checks for valid cluster store structure at some point.
        Ok(Self {
            store_path,
            clusters,
        })
    }
}

/// All possible error types for cluster store interaction.
#[derive(Debug, thiserror::Error)]
pub enum ClusterStoreError {
    #[error(transparent)]
    Glob(#[from] glob::GlobError),

    #[error(transparent)]
    GlobPattern(#[from] glob::PatternError),

    #[error(transparent)]
    Cluster(#[from] crate::cluster::domain::ClusterError),
}

/// Friendly result alias :3
type Result<T, E = ClusterStoreError> = std::result::Result<T, E>;
