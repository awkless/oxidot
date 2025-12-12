// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

//! Sparse checkout rule handling.
//!
//! Utilities to manage the sparsity rules being used to deploy a cluster's
//! file content.
//!
//! # Why Sparse Checkout?
//!
//! Git comes with a cool feature called __sparse checkout__. It allows the
//! user to reduce their work tree to a subset of tracked files. What gets
//! included in this reduced work tree is determined by a set of
//! __sparsity rules__. A sparsity rule is just a pattern of characters that
//! match tracked files for inclusion into the reduced work tree. The syntax
//! of a sparsity rule is the same as the gitignore syntax, with the exception
//! that the semantics are inverted. Thus, unlike gitignore semantics, sparsity
//! semantics do not include any tracked files by default such that each
//! sparsity rule determines what to _include_ instead of what to exclude.
//!
//! Sparse checkout operates in one of two modes: cone or non-cone mode. These
//! operation modes simply reduce the allowable set of sparsity rule patterns
//! that can be used. Cone mode only allows for the usage of sparsity rule
//! patterns that include directories. Non-cone mode allows usage of the
//! _entire_ sparsity rule pattern set. By default Git uses cone mode.
//!
//! Oxidot employs sparse checkout as the backbone of its file deployment
//! feature for clusters in the cluster store. When using sparse checkout with
//! bare-alias repositories, file content can be directly deployed to a work
//! tree alias without needing to manually symlink, copy, or move it. This also
//! has the added benefit of allowing Git itself to keep track of these
//! deployed files without needing to modify the commit history of any given
//! cluster in the cluster store.
//!
//! # Sparse Checkout Configuration File Layout
//!
//! Git does not provide a way to define sparsity rules in the same fashion as
//! gitignore rules, where you can put them in a special hidden file at the
//! top-level of a repository. Instead, sparsity rules are directly stored
//! in the gitdir at `$gitdir/info/sparse-checkout`. If this file does not
//! exist, then Git will not deploy any tracked files to the work tree alias.
//!
//! Git interprets sparsity rules on a per line basis. Thus, each sparsity rule
//! must be placed on its own separate line. Git validates all sparsity rules
//! according to the current mode that sparse checkout has been set to.
//!
//! # Pitfalls
//!
//! The main problem with non-cone mode is its runtime of O(N*M) where N is
//! number of sparsity rules to check, and M is the number of paths to check
//! against. Not only that, but non-cone mode is considered a deprecated
//! feature mode for sparse checkout. The maintainers of sparse checkout have
//! promised to not to remove non-cone mode, but non-cone mode will not be
//! getting the new feature updates that cone mode will get.
//!
//! By default Oxidot uses non-cone mode for sparse checkout. We prefer to give
//! the user full access to the sparsity rule pattern set to make it easier to
//! deploy any component of a cluster. Obviously, some user's may take issue
//! with Oxidot's deployment feature being based on a deprecated feature. So, we
//! give them a choice between the two modes for sparse checkout.
//!
//! # See Also
//!
//! - [Man page sparse checkout](https://git-scm.com/docs/git-sparse-checkout)

use crate::config::WorkTreeAlias;

use ignore::gitignore::GitignoreBuilder;
use std::{
    collections::HashSet,
    fmt::Write,
    fs::{read_to_string, write, OpenOptions},
    path::{Path, PathBuf},
};

/// Manage sparsity rules in sparse checkout file.
///
/// Provides methods to read, write, and match sparsity rules to target file
/// paths.
#[derive(Clone, Debug)]
pub struct SparsityDrafter<M>
where
    M: SparsityMatcher,
{
    sparse_path: PathBuf,
    matcher: M,
}

impl<M> SparsityDrafter<M>
where
    M: SparsityMatcher,
{
    /// Construct new sparsity rule drafter.
    ///
    /// Creats the sparse checkout configuration file if it does not already
    /// exist yet.
    ///
    /// # Errors
    ///
    /// - Return [`SparseError::CreateSparseFile`] if sparse Checkout
    ///   configuration file cannot be created if missing.
    pub fn new(gitdir: impl Into<PathBuf>, matcher: M) -> Result<Self> {
        let sparse_path = gitdir.into().join("info").join("sparse-checkout");

        // INVARIANT: Create sparse checkout file if needed.
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&sparse_path)
            .map_err(|err| SparseError::CreateSparseFile {
                source: err,
                sparse_path: sparse_path.clone(),
            })?;

        Ok(Self {
            sparse_path,
            matcher,
        })
    }

    /// Insert a set of sparsity rules.
    ///
    /// Take set of sparsity rules, and append them to existing rule set in
    /// configuration file. Will not overwrite existing sparsity rules. Ensures
    /// that each newly added rule is on its own separate line. Any duplicate
    /// rules will be removed, ensuring that all rules in the sparse
    /// configuration file are unique.
    ///
    /// # Errors
    ///
    /// - Return [`SparseError::ReadSparseFile`] if configuration file cannot
    ///   be read from.
    /// - Return [`SparseError::WriteSparseFile`] if configuration file cannot
    ///   be written to.
    pub fn insert_rules(&self, rules: impl IntoIterator<Item = impl Into<String>>) -> Result<()> {
        let mut rule_set =
            read_to_string(&self.sparse_path).map_err(|err| SparseError::ReadSparseFile {
                source: err,
                sparse_path: self.sparse_path.clone(),
            })?;

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

        write(&self.sparse_path, rule_set.as_bytes()).map_err(|err| {
            SparseError::WriteSparseFile {
                source: err,
                sparse_path: self.sparse_path.clone(),
            }
        })?;

        Ok(())
    }

    /// Remove a set of sparsity rules.
    ///
    /// Finds sparsity rules that need to be removed from configuration file
    /// based on provided removal list. Rules that do not match will be left
    /// alone.
    ///
    /// # Errors
    ///
    /// - Return [`SparseError::ReadSparseFile`] if configuration file cannot
    ///   be read from.
    /// - Return [`SparseError::WriteSparseFile`] if configuration file cannot
    ///   be written to.
    pub fn remove_rules(&self, rules: impl IntoIterator<Item = impl Into<String>>) -> Result<()> {
        let content =
            read_to_string(&self.sparse_path).map_err(|err| SparseError::ReadSparseFile {
                source: err,
                sparse_path: self.sparse_path.clone(),
            })?;

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

        write(&self.sparse_path, result.as_bytes()).map_err(|err| {
            SparseError::WriteSparseFile {
                source: err,
                sparse_path: self.sparse_path.clone(),
            }
        })?;

        Ok(())
    }

    /// List out current rule set.
    ///
    /// # Errors
    ///
    /// - Return [`SparseError::ReadSparseFile`] if configuration file cannot
    ///   be read from.
    pub fn current_rules(&self) -> Result<Vec<String>> {
        let content =
            read_to_string(&self.sparse_path).map_err(|err| SparseError::ReadSparseFile {
                source: err,
                sparse_path: self.sparse_path.clone(),
            })?;

        Ok(content.lines().map(String::from).collect())
    }

    /// Clear out _all_ sparsity rules.
    ///
    /// # Errors
    ///
    /// - Return [`SparseError::WriteSparseFile`] if configuration file cannot
    ///   be written to.
    pub fn clear_rules(&self) -> Result<()> {
        write(&self.sparse_path, b"").map_err(|err| SparseError::WriteSparseFile {
            source: err,
            sparse_path: self.sparse_path.clone(),
        })
    }

    pub fn path_matches(&self, path: impl AsRef<Path>) -> bool {
        self.matcher
            .path_matches(path.as_ref(), self.current_rules().unwrap().as_ref())
    }
}

/// Match sparsity rules.
///
/// Model ways to match sparsity rules to various stuff.
pub trait SparsityMatcher {
    /// Match a path to a listing of sparsity rules.
    ///
    /// Compare path to each rule to see if it matches.
    fn path_matches(&self, path: &Path, rules: &[String]) -> bool;
}

/// A sparsity ruler matcher that inverts gitignore semantics.
///
/// Takes a gitignore rule parser, and inverts incoming patterns to match
/// sparsity patterns instead. Is this very lazy and hacky way to interpret
/// sparsity patterns? Yes. Does it work? Also yes.
///
/// > If it looks stupid but it works, then it wasn't stupid.
/// > - Random Engineer
#[derive(Debug)]
pub struct InvertedGitignore {
    work_tree_alias: WorkTreeAlias,
}

impl InvertedGitignore {
    /// Construct new inverted gitignore matcher.
    ///
    /// Base pattern matching relative to target work tree alias.
    pub fn new(work_tree_alias: impl Into<WorkTreeAlias>) -> Self {
        Self {
            work_tree_alias: work_tree_alias.into(),
        }
    }
}

impl SparsityMatcher for InvertedGitignore {
    /// Match path to listing of sparsity rules.
    ///
    /// Matches files and directories in one shot. Takes longer, but ensures
    /// that any file patttern is checked along with any directory pattern.
    fn path_matches(&self, path: &Path, rules: &[String]) -> bool {
        let mut builder = GitignoreBuilder::new(self.work_tree_alias.as_path());
        // INVARIANT: Invert gitignore logic.
        //   - Ignore everything by default.
        //   - Invert '!' to mean to unignore.
        //   - Invert any rule without '!' to mean ignore.
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
            .matched_path_or_any_parents(path, path.is_dir())
            .is_ignore()
    }
}

/// Sparsity rule management error types.
#[derive(Debug, thiserror::Error)]
pub enum SparseError {
    /// Sparse configuration file cannot be crated when missing.
    #[error("failed to create sparse file at {:?}", sparse_path.display())]
    CreateSparseFile {
        #[source]
        source: std::io::Error,
        sparse_path: PathBuf,
    },

    /// Sparse configuration file cannot be read from.
    #[error("failed to read from sparse file at {:?}", sparse_path.display())]
    ReadSparseFile {
        #[source]
        source: std::io::Error,
        sparse_path: PathBuf,
    },

    /// Sparse configuration file cannot be written to.
    #[error("failed to write to sparse file at {:?}", sparse_path.display())]
    WriteSparseFile {
        #[source]
        source: std::io::Error,
        sparse_path: PathBuf,
    },
}

/// Friendly result alias :3
type Result<T, E = SparseError> = std::result::Result<T, E>;
