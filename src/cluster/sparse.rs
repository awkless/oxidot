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
    fmt::{Display, Formatter, Result as FmtResult},
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
    /// - Return [`Error::CreateSparseFile`] if sparse Checkout
    ///   configuration file cannot be created if missing.
    pub fn new(gitdir: impl Into<PathBuf>, matcher: M) -> Result<Self> {
        let sparse_path = gitdir.into().join("info").join("sparse-checkout");

        // INVARIANT: Create sparse checkout file if needed.
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&sparse_path)
            .map_err(|err| Error::CreateSparseFile {
                source: err,
                sparse_path: sparse_path.clone(),
            })?;

        Ok(Self {
            sparse_path,
            matcher,
        })
    }

    /// Edit sparsity rules.
    ///
    /// Read current rule set into [`SparsityEdit`] instance, and directly
    /// edit each rule before writing the results back into the sparse checkout
    /// configuration file.
    ///
    /// # Errors
    ///
    /// - Return [`Error::ReadSparseFile`] if sparse checkout
    ///   configuration file cannot be read.
    /// - Return [`Error::WriteSparseFile`] if rules cannot be written to
    ///   sparse checkout configuration file.
    pub fn edit<E>(&self, editor: E) -> Result<()>
    where
        E: FnOnce(&mut SparsityEdit),
    {
        let content = read_to_string(&self.sparse_path).map_err(|err| Error::ReadSparseFile {
            source: err,
            sparse_path: self.sparse_path.clone(),
        })?;

        let mut rules = SparsityEdit::from(content);
        editor(&mut rules);

        if !rules.changed {
            return Ok(());
        }

        write(&self.sparse_path, rules.to_string().as_bytes()).map_err(|err| {
            Error::WriteSparseFile {
                source: err,
                sparse_path: self.sparse_path.clone(),
            }
        })?;

        Ok(())
    }

    /// List current sparsity rule set.
    ///
    /// # Errors
    ///
    /// - Return [`Error::ReadSparseFile`] if sparse checkout
    ///   configuration file cannot be read.
    pub fn current_rules(&self) -> Result<Vec<String>> {
        read_to_string(&self.sparse_path)
            .map_err(|err| Error::ReadSparseFile {
                source: err,
                sparse_path: self.sparse_path.clone(),
            })
            .map(|content| content.lines().map(str::to_owned).collect::<Vec<_>>())
    }

    /// Match file path to current sparsity rules relative to a work tree alias.
    ///
    /// Matches file path against all currently avaiable sparsity rules
    /// relative the a target work tree alias using given [`SparsityMatcher`].
    pub fn path_matches(&self, work_tree_alias: &WorkTreeAlias, path: impl AsRef<Path>) -> bool {
        self.matcher
            .path_matches(work_tree_alias, path, self.current_rules().unwrap())
    }
}

/// Sparsity rule editor.
///
/// # Invariant
///
/// - No duplicate sparsity rules.
/// - Rule insertion does not overwrite existing rules.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SparsityEdit {
    rules: HashSet<String>,
    changed: bool,
}

impl SparsityEdit {
    /// Construct new sparsity rule editor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a sparsity rule.
    pub fn insert_rule(&mut self, rule: impl Into<String>) {
        let rule = rule.into();
        if self.rules.insert(rule) {
            self.changed = true;
        }
    }

    /// Insert a listing of sparsity rules.
    pub fn insert_rules(&mut self, rules: impl IntoIterator<Item = impl Into<String>>) {
        for rule in rules {
            let rule = rule.into();
            if self.rules.insert(rule) {
                self.changed = true;
            }
        }
    }

    /// Remove a sparsity rule
    pub fn remove_rule(&mut self, rule: impl AsRef<str>) {
        if self.rules.remove(rule.as_ref()) {
            self.changed = true;
        }
    }

    /// Remove a listing of sparsity rules.
    pub fn remove_rules(&mut self, rules: impl IntoIterator<Item = impl AsRef<str>>) {
        for rule in rules {
            if self.rules.remove(rule.as_ref()) {
                self.changed = true;
            }
        }
    }

    /// Clear all sparsity rules.
    pub fn clear_rules(&mut self) {
        if !self.rules.is_empty() {
            self.rules.clear();
            self.changed = true;
        }
    }
}

impl Display for SparsityEdit {
    fn fmt(&self, fmt: &mut Formatter) -> FmtResult {
        if self.rules.is_empty() {
            return write!(fmt, "");
        }

        let mut rules: Vec<_> = self.rules.iter().collect();
        rules.sort();

        let mut out = String::new();
        for rule in rules {
            out.push_str(rule);
            out.push('\n');
        }

        write!(fmt, "{out}")
    }
}

impl From<String> for SparsityEdit {
    fn from(content: String) -> Self {
        let rules = content.lines().map(str::to_owned).collect::<HashSet<_>>();

        Self {
            rules,
            changed: false,
        }
    }
}

impl From<&str> for SparsityEdit {
    fn from(content: &str) -> Self {
        let rules = content.lines().map(str::to_owned).collect::<HashSet<_>>();

        Self {
            rules,
            changed: false,
        }
    }
}

/// Match sparsity rules.
///
/// Model ways to match sparsity rules to various stuff.
pub trait SparsityMatcher: Send + Sync + 'static {
    /// Match a path to a listing of sparsity rules.
    ///
    /// Compare path to each rule to see if it matches.
    fn path_matches(
        &self,
        work_tree_alias: &WorkTreeAlias,
        path: impl AsRef<Path>,
        rules: impl IntoIterator<Item = impl Into<String>>,
    ) -> bool;
}

/// A sparsity rule matcher that inverts gitignore semantics.
///
/// Takes a gitignore rule parser, and inverts incoming patterns to match
/// sparsity patterns instead. Is this very lazy and hacky way to interpret
/// sparsity patterns? Yes. Does it work? Also yes.
///
/// > If it looks stupid but it works, then it wasn't stupid.
/// > - Random Engineer
#[derive(Debug, Default)]
pub struct InvertedGitignore;

impl InvertedGitignore {
    /// Construct new inverted gitignore matcher.
    ///
    /// Base pattern matching relative to target work tree alias.
    pub fn new() -> Self {
        Self
    }
}

impl SparsityMatcher for InvertedGitignore {
    /// Match path to listing of sparsity rules.
    ///
    /// Matches files and directories in one shot. Takes longer, but ensures
    /// that any file patttern is checked along with any directory pattern.
    fn path_matches(
        &self,
        work_tree_alias: &WorkTreeAlias,
        path: impl AsRef<Path>,
        rules: impl IntoIterator<Item = impl Into<String>>,
    ) -> bool {
        let mut builder = GitignoreBuilder::new(work_tree_alias.as_path());
        // INVARIANT: Invert gitignore logic.
        //   - Ignore everything by default.
        //   - Invert '!' to mean to unignore.
        //   - Invert any rule without '!' to mean ignore.
        builder.add_line(None, "/*").unwrap();
        for rule in rules.into_iter().map(Into::into) {
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
            .matched_path_or_any_parents(path.as_ref(), path.as_ref().is_dir())
            .is_ignore()
    }
}

/// Sparsity rule management error types.
#[derive(Debug, thiserror::Error)]
pub enum Error {
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
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    #[test]
    fn sparsity_edit_rule_insertion() {
        let mut editor = SparsityEdit::default();

        editor.insert_rule("/.vim/");
        editor.insert_rule("!*.aux");
        editor.insert_rule("/**/bin");
        let result = editor.to_string();
        let expect = indoc! {r#"
            !*.aux
            /**/bin
            /.vim/
        "#};
        assert_eq!(result, expect);

        editor.insert_rules(["cluster.toml", "/.ssh", "/*"]);
        let result = editor.to_string();
        let expect = indoc! {r#"
            !*.aux
            /*
            /**/bin
            /.ssh
            /.vim/
            cluster.toml
        "#};
        assert_eq!(result, expect);

        // No duplication.
        editor.insert_rule("/.vim/");
        let result = editor.to_string();
        assert_eq!(result, expect);
    }

    #[test]
    fn sparsity_edit_rule_removal() {
        let rule_set = indoc! {r#"
            !*.aux
            /*
            /**/bin
            /.ssh
            /.vim/
            cluster.toml
        "#};
        let mut editor = SparsityEdit::from(rule_set);

        editor.remove_rule("/**/bin");
        editor.remove_rule("cluster.toml");
        editor.remove_rule("/.ssh");
        let result = editor.to_string();
        let expect = indoc! {r#"
            !*.aux
            /*
            /.vim/
        "#};
        assert_eq!(result, expect);
    }

    #[test]
    fn sparsity_edit_clear_rules() {
        let rule_set = indoc! {r#"
            !*.aux
            /*
            /**/bin
            /.ssh
            /.vim/
            cluster.toml
        "#};
        let mut editor = SparsityEdit::from(rule_set);

        editor.clear_rules();
        let result = editor.to_string();
        let expect = String::new();
        assert_eq!(result, expect);
    }
}
