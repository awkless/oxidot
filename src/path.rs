// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

//! Path resolution utilities.
//!
//! Determine relevent path information for external files that need to be
//! interacted with, or managed in some way.

use std::path::PathBuf;

/// Determine absolute path to user's home directory.
///
/// Does not check if the path returned actually exists.
///
/// # Errors
///
/// - Return [`NoWayHome`] if home directory path cannot be determined.
pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or(NoWayHome)
}

/// Determine default absolute path to cluster store directory.
///
/// Uses XDG Base Directory path `$XDG_DATA_HOME/oxidot-store` as the default
/// absolute path for a cluster store. Does not check if the path returned
/// actually exists.
///
/// # Errors
///
/// - Return [`NoWayHome`] if home directory path cannot be determined.
///
/// # See Also
///
/// - [XDG Base Directory](https://wiki.archlinux.org/title/XDG_Base_Directory)
pub fn default_cluster_store_dir() -> Result<PathBuf> {
    dirs::data_dir()
        .map(|path| path.join("oxidot-store"))
        .ok_or(NoWayHome)
}

/// No way to determine user's home directory.
///
/// # See Also
///
/// - [`dirs::home_dir`](https://docs.rs/dirs/latest/dirs/fn.home_dir.html)
#[derive(Clone, Debug, thiserror::Error)]
#[error("cannot determine absolute path to user's home directory")]
pub struct NoWayHome;

/// Friendly result alias :3
pub type Result<T, E = NoWayHome> = std::result::Result<T, E>;
