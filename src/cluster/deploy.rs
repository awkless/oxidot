// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

//! Cluster deployment logic.
//!
//! Utilities to handle cluster deployment logic. Given that cluster's are
//! bare-alias, they are technically always considered to be deployed once
//! created. Thus, any piece of logic that must interact with a cluster is
//! generally considered to be deployment logic, e.g., staging files, committing
//! files, applying sparsity rules, getting status information, etc., are all
//! considered to be deployment logic.

use crate::{
    cluster::sparse::{InvertedGitignore, SparsityDrafter},
    config::WorkTreeAlias,
};

use git2::{Blob, IndexEntry, IndexTime, ObjectType, Repository};
use std::{
    collections::{HashSet, VecDeque},
    ffi::{OsStr, OsString},
    fmt::{Debug, Formatter, Result as FmtResult},
    path::{Path, PathBuf},
    process::Command,
};
use tracing::{debug, info, instrument, warn};

/// Core deployment logic.
pub trait Deployment {
    /// Check if cluster is empty.
    fn is_empty(&self) -> bool;

    /// Get contents of file from cluster.
    fn cat_file(&self, path: impl AsRef<Path>) -> Result<String>;

    /// Stage and commit content into cluster.
    fn stage_and_commit(
        &self,
        filename: impl AsRef<Path>,
        contents: impl AsRef<str>,
        message: impl AsRef<str>,
    ) -> Result<()>;

    /// Deploy tracked files to work tree alias that match sparsity rules.
    fn deploy_with_rules(
        &self,
        work_tree_alias: &WorkTreeAlias,
        rules: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<()>;

    /// Undeploy tracked files from work tree alias that match sparsity rules.
    fn undeploy_with_rules(
        &self,
        work_tree_alias: &WorkTreeAlias,
        rules: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<()>;

    /// Deploy all tracked files to work tree alias.
    fn deploy_all(&self, work_tree_alias: &WorkTreeAlias) -> Result<()>;

    /// Undeploy all tracked files from work tree alias.
    fn undeploy_all(&self, work_tree_alias: &WorkTreeAlias) -> Result<()>;

    /// Check if cluster has deployed any tracked files to work tree alias.
    fn is_deployed(&self, work_tree_alias: &WorkTreeAlias) -> bool;

    /// Block process to intract with cluster through Git.
    fn gitcall_interactive(
        &self,
        work_tree_alias: &WorkTreeAlias,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<()>;

    /// Interact with cluster through Git without blocking process.
    fn gitcall_non_interactive(
        &self,
        work_tree_alias: &WorkTreeAlias,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<String>;
}

/// Cluster deployment logic backed by libgit2.
pub struct Git2Deployer {
    repository: Repository,
    sparsity: SparsityDrafter<InvertedGitignore>,
}

impl Git2Deployer {
    /// Construct new libgit2 cluster deployer.
    ///
    /// Always ensures that these configuration settings are enabled by
    /// default:
    ///
    /// 1. Do not show untracked files.
    /// 2. Always enable sparse checkout.
    /// 3. Allow changes to work tree alias outside of sparsity rules.
    ///
    /// # Error
    ///
    /// - Return [`DeployError::Git2`] if configuration settings cannot be set
    ///   for cluster.
    pub fn new(
        repository: Repository,
        sparsity: SparsityDrafter<InvertedGitignore>,
    ) -> Result<Self> {
        let deployer = Self {
            repository,
            sparsity,
        };

        // INVARIANT: Do not show untracked files.
        let mut config = deployer.repository.config()?;
        if deployer.get_config_value(&config, "status.showUntrackedFiles")? != Some("no".into()) {
            config.set_str("status.showUntrackedFiles", "no")?;
        }

        // INVARIANT: Always enable sparse checkout.
        if deployer.get_config_value(&config, "core.sparseCheckout")? != Some("true".into()) {
            config.set_str("core.sparseCheckout", "true")?;
        }

        // INVARIANT: Allow changes to work tree alias outside of sparsity rules.
        if deployer.get_config_value(&config, "advice.updateSparsePath")? != Some("true".into()) {
            config.set_str("advice.updateSparsePath", "false")?;
        }

        Ok(deployer)
    }

    fn get_config_value(&self, config: &git2::Config, key: &str) -> Result<Option<String>> {
        match config.get_entry(key) {
            Ok(entry) => Ok(entry.value().map(|v| v.to_string())),
            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(err) => Err(DeployError::Git2(err)),
        }
    }

    fn find_blob(&self, path: impl AsRef<Path>) -> Result<Blob<'_>> {
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
                    // INVARIANT: Hit a blob? Check it!
                    Some(ObjectType::Blob) => {
                        let full_path = path.join(bytes_to_path(tree_entry.name_bytes()));
                        if &full_path == &path {
                            return Ok(tree_entry.to_object(&self.repository)?.peel_to_blob()?);
                        }
                        entries.push(full_path);
                    }
                    _ => continue,
                }
            }
        }

        Err(DeployError::BlobNotFound {
            path: path.as_ref().into(),
        })
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

    fn expand_bin_args(
        &self,
        work_tree_alias: &WorkTreeAlias,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Vec<OsString> {
        let gitdir = self.repository.path().to_string_lossy().into_owned().into();
        let path_args: Vec<OsString> = vec![
            "--git-dir".into(),
            gitdir,
            "--work-tree".into(),
            work_tree_alias.to_os_string(),
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
    fn sync_sparse_with_new_files(
        &self,
        work_tree_alias: &WorkTreeAlias,
        new_files: &[PathBuf],
    ) -> Result<()> {
        let mut new_rules = Vec::new();
        for path in new_files {
            let full_path = work_tree_alias.as_path().join(path);

            debug!(
                "checking if {} matches existing sparse rules",
                path.display()
            );

            if !self.sparsity.path_matches(work_tree_alias, &full_path) {
                debug!("adding new sparse rule for {}", path.display());
                new_rules.push(path.display().to_string());
            } else {
                debug!("{} already covered by existing rules", path.display());
            }
        }

        if !new_rules.is_empty() {
            info!("adding {} new sparse rules", new_rules.len());
            self.sparsity
                .edit(|editor| editor.insert_rules(&new_rules))?;
            syscall_non_interactive("git", self.expand_bin_args(work_tree_alias, ["checkout"]))?;
        }

        Ok(())
    }
}

impl Debug for Git2Deployer {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        writeln!(f, "gitdir: {:?}", self.repository.path().display())
    }
}

impl Deployment for Git2Deployer {
    /// Get contents of file from cluster.
    ///
    /// Searches for a blob that matches the target path in the cluster's
    /// index. That blob is then converted into a valid string and returned.
    ///
    /// # Errors
    ///
    /// - Return [`DeployError::Git2`] if any libgit2 operations fail.
    fn cat_file(&self, path: impl AsRef<Path>) -> Result<String> {
        let blob = self.find_blob(path.as_ref())?;

        Ok(String::from_utf8_lossy(blob.content()).into_owned())
    }

    /// Stage and commit a new file into cluster.
    ///
    /// Add a new file with content into the cluster's index directly and
    /// commit it with a message. The commit is placed after the most recent
    /// commit.
    ///
    /// # Errors
    ///
    /// - Return [`DeployError::Git2`] if any libgit2 operations fail.
    fn stage_and_commit(
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

    /// Check if cluster is empty.
    ///
    /// A cluster is empty if it does not have a HEAD with a valid commit.
    ///
    /// # Errors
    ///
    /// - Return [`DeployError::Git2`] if any libgit2 operations fail.
    fn is_empty(&self) -> bool {
        self.repository
            .head()
            .ok()
            .and_then(|head| head.target())
            .and_then(|oid| self.repository.find_commit(oid).ok())
            .is_none()
    }

    /// Deploy tracked files to work tree alias that match sparsity rules.
    ///
    /// Adds target rules to match tracked files, and updates cluster's index.
    ///
    /// # Errors
    ///
    /// - Return [`DeployError::Syscall`] if changes to sparsity rule set fails
    ///   to be applied.
    /// - Return [`DeployError::Sparse`] if sparsity rule manipulation fails..
    /// - Return [`DeployError::Git2`] if cluster index operations fail.
    #[instrument(skip(self, work_tree_alias, rules), level = "debug")]
    fn deploy_with_rules(
        &self,
        work_tree_alias: &WorkTreeAlias,
        rules: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<()> {
        info!("deploy {:?}", self.repository.path().display());
        if self.is_empty() {
            warn!("cluster {:?} is empty", self.repository.path().display());
            return Ok(());
        }

        self.sparsity.edit(|editor| editor.insert_rules(rules))?;
        let output = self.gitcall_non_interactive(work_tree_alias, ["checkout"])?;
        info!("{output}");

        Ok(())
    }

    /// Undeploy tracked files from work tree alias that match sparsity rules.
    ///
    /// Removes target rules from sparse checkout configuration file, and
    /// applies updates to cluster's index.
    ///
    /// # Errors
    ///
    /// - Return [`DeployError::Syscall`] if changes to sparsity rule set fails
    ///   to be applied.
    /// - Return [`DeployError::Sparse`] if sparsity rule manipulation fails..
    /// - Return [`DeployError::Git2`] if cluster index operations fail.
    #[instrument(skip(self, work_tree_alias, rules), level = "debug")]
    fn undeploy_with_rules(
        &self,
        work_tree_alias: &WorkTreeAlias,
        rules: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<()> {
        info!("undeploy {:?}", self.repository.path().display());
        if self.is_empty() {
            warn!("cluster {:?} is empty", self.repository.path().display());
            return Ok(());
        }

        self.sparsity.edit(|editor| editor.remove_rules(rules))?;
        let output = self.gitcall_non_interactive(work_tree_alias, ["checkout"])?;
        info!("{output}");

        Ok(())
    }

    /// Deploy all tracked files of cluster to work tree alias.
    ///
    /// Replaces entire sparsity rule set with one rule: "/*". Applies this
    /// new and only rule to cluster's index.
    ///
    /// # Errors
    ///
    /// - Return [`DeployError::Syscall`] if changes to sparsity rule set fails
    ///   to be applied.
    /// - Return [`DeployError::Sparse`] if sparsity rule manipulation fails..
    /// - Return [`DeployError::Git2`] if cluster index operations fail.
    #[instrument(skip(self), level = "debug")]
    fn deploy_all(&self, work_tree_alias: &WorkTreeAlias) -> Result<()> {
        info!("deploy all of {:?}", self.repository.path().display());
        if self.is_empty() {
            warn!("cluster {:?} is empty", self.repository.path().display());
            return Ok(());
        }

        self.sparsity.edit(|editor| {
            editor.clear_rules();
            editor.insert_rule("/*");
        })?;
        let output = self.gitcall_non_interactive(work_tree_alias, ["checkout"])?;
        info!("{output}");

        Ok(())
    }

    /// Undeploy all tracked files of cluster from work tree alias.
    ///
    /// Simply clears entire sparsity rule set, and applies this change to the
    /// cluster's index.
    ///
    /// # Panics
    ///
    /// - May panic if spasrity rule parsing fails.
    ///
    /// # Errors
    ///
    /// - Return [`DeployError::Syscall`] if changes to sparsity rule set fail
    ///   to be applied.
    /// - Return [`DeployError::Sparse`] if sparsity rule manipulation fails..
    /// - Return [`DeployError::Git2`] if cluster index operations fail.
    #[instrument(skip(self), level = "debug")]
    fn undeploy_all(&self, work_tree_alias: &WorkTreeAlias) -> Result<()> {
        if !self.is_deployed(work_tree_alias) {
            warn!(
                "cluster {:?} already undeployed in full",
                self.repository.path().display()
            );
            return Ok(());
        }

        self.sparsity.edit(|editor| editor.clear_rules())?;
        let output = self.gitcall_non_interactive(work_tree_alias, ["checkout"])?;
        info!("{output}");

        Ok(())
    }

    /// Check if cluster has deployed tracked files to work tree alias.
    ///
    /// Performs a first occurance search through each tracked file in the
    /// cluster such that the tracked file that matches a sparsity rule, and
    /// exists in the cluster's work tree alias, means that the cluster is
    /// deployed. Otherwise, the cluster is not deployed.
    ///
    /// # Panics
    ///
    /// - May panic if spasrity rule parsing fails.
    fn is_deployed(&self, work_tree_alias: &WorkTreeAlias) -> bool {
        if self.is_empty() {
            return false;
        }

        let rules = match self.sparsity.current_rules() {
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
            let full_path = work_tree_alias.as_path().join(&entry);
            if full_path.exists() && self.sparsity.path_matches(work_tree_alias, &full_path) {
                return true;
            }
        }

        false
    }

    /// Interact with cluster directly through Git via current process.
    ///
    /// Preserves consistency between sparsity rules and index when caller
    /// uses commands like git-add, git-rm, etc. Blocks current process to allow
    /// for direct interaction with cluster through Git.
    ///
    /// # Errors
    ///
    /// - Return [`DeployError::Syscall`] if system call to Git fails, or Git
    ///   itself fails.
    /// - Return [`DeployError::Git2`] if any operation on the cluster's index
    ///   fails.
    fn gitcall_interactive(
        &self,
        work_tree_alias: &WorkTreeAlias,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<()> {
        let index_before = self.get_staged_paths()?;
        syscall_interactive("git", self.expand_bin_args(work_tree_alias, args))?;
        let index_after = self.get_staged_paths()?;

        // INVARIANT: Sync sparsity rules with index if and only if the index itself has changed.
        let newly_added: Vec<PathBuf> = index_after.difference(&index_before).cloned().collect();
        if !newly_added.is_empty() {
            self.sync_sparse_with_new_files(work_tree_alias, &newly_added)?;
        }

        Ok(())
    }

    /// Interact with cluster directly through Git via external process.
    ///
    /// Does not block current process. Instead the system call is made via
    /// external process whose output to stdout and stderr is returned
    /// together as a [`String`].
    ///
    /// # Errors
    ///
    /// - Return [`DeployError::Syscall`] if system call to Git fails, or Git
    ///   itself fails.
    /// - Return [`DeployError::Git2`] if any operation on the cluster's index
    ///   fails.
    fn gitcall_non_interactive(
        &self,
        work_tree_alias: &WorkTreeAlias,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<String> {
        syscall_non_interactive("git", self.expand_bin_args(work_tree_alias, args))
    }
}

fn syscall_interactive(
    cmd: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> Result<()> {
    let status = Command::new(cmd.as_ref()).args(args).spawn()?.wait()?;
    if !status.success() {
        return Err(DeployError::Syscall(std::io::Error::other(format!(
            "command {:?} failed",
            cmd.as_ref()
        ))));
    }

    Ok(())
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
        message.push_str(format!("stdout: {stderr}").as_str());
    }

    // INVARIANT: Chomp trailing newlines.
    let message = message
        .strip_suffix("\r\n")
        .or(message.strip_suffix('\n'))
        .map(ToString::to_string)
        .unwrap_or(message);

    if !output.status.success() {
        return Err(DeployError::Syscall(std::io::Error::other(format!(
            "command {:?} failed:\n{message}",
            cmd.as_ref()
        ))));
    }

    Ok(message)
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

/// Deployment logic error types.
#[derive(Debug, thiserror::Error)]
pub enum DeployError {
    /// Sparse checkout configuration file manipulation fails.
    #[error(transparent)]
    Sparse(#[from] crate::cluster::sparse::SparseError),

    /// Target blob in cluster's index cannot be found.
    #[error("cannot find file blob for {:?}", path.display())]
    BlobNotFound { path: PathBuf },

    /// Operations from libgit2 fail.
    #[error(transparent)]
    Git2(#[from] git2::Error),

    /// System calls fail.
    #[error(transparent)]
    Syscall(#[from] std::io::Error),
}

/// Friendly result alias :3
type Result<T, E = DeployError> = std::result::Result<T, E>;
