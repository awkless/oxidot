// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

use git2::Repository;
use std::path::Path;
use anyhow::Result;

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
    repo: Repository,
}

impl Cluster {
    pub fn try_new_init(path: impl AsRef<Path>) -> Result<Self> {
        todo!();
    }

    pub fn try_new_open(path: impl AsRef<Path>) -> Result<Self> {
        todo!();
    }

    pub fn try_new_clone(url: impl AsRef<str>, path: impl AsRef<Path>) -> Result<Self> {
        todo!();
    }
}
