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

use crate::cluster::domain::Cluster;

use std::{collections::HashMap, path::{PathBuf, Path}};

pub struct ClusterStore {
    store_path: PathBuf,
    clusters: HashMap<String, Cluster>,
}

impl ClusterStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        todo!();
    }
}

/// All possible error types for cluster store interaction.
#[derive(Debug, thiserror::Error)]
pub enum ClusterStoreError {
    // TODO: Add errors here!
}

/// Friendly result alias :3
type Result<T, E = ClusterStoreError> = std::result::Result<T, E>;
