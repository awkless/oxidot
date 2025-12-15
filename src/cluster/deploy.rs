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

use git2::Repository;
use std::{
    ffi::{OsStr, OsString},
    path::Path,
    process::Command,
};

pub trait Deployment {
    fn cat_file(&self, path: &Path) -> Result<String>;

    fn deploy_with_rules(
        &self,
        work_tree_alias: &WorkTreeAlias,
        rules: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<()>;

    fn undeploy_with_rules(
        &self,
        work_tree_alias: &WorkTreeAlias,
        rules: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<()>;

    fn deploy_all(&self, work_tree_alias: &WorkTreeAlias) -> Result<()>;

    fn undeploy_all(&self, work_tree_alias: &WorkTreeAlias) -> Result<()>;

    fn is_deployed(&self, work_tree_alias: &WorkTreeAlias) -> bool;

    fn gitcall_interactive(
        &self,
        work_tree_alias: &WorkTreeAlias,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<()>;

    fn gitcall_non_interactive(
        &self,
        work_tree_alias: &WorkTreeAlias,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<String>;
}

pub struct Git2Deployer {
    repository: Repository,
    sparsity: SparsityDrafter<InvertedGitignore>,
}

impl Git2Deployer {
    pub fn new(repository: Repository, sparsity: SparsityDrafter<InvertedGitignore>) -> Self {
        Self {
            repository,
            sparsity,
        }
    }
}

impl Deployment for Git2Deployer {
    fn cat_file(&self, path: &Path) -> Result<String> {
        todo!();
    }

    fn deploy_with_rules(
        &self,
        work_tree_alias: &WorkTreeAlias,
        rules: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<()> {
        todo!();
    }

    fn undeploy_with_rules(
        &self,
        work_tree_alias: &WorkTreeAlias,
        rules: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<()> {
        todo!();
    }

    fn deploy_all(&self, work_tree_alias: &WorkTreeAlias) -> Result<()> {
        todo!();
    }

    fn undeploy_all(&self, work_tree_alias: &WorkTreeAlias) -> Result<()> {
        todo!();
    }

    fn is_deployed(&self, work_tree_alias: &WorkTreeAlias) -> bool {
        todo!();
    }

    fn gitcall_interactive(
        &self,
        work_tree_alias: &WorkTreeAlias,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<()> {
        todo!();
    }

    fn gitcall_non_interactive(
        &self,
        work_tree_alias: &WorkTreeAlias,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<String> {
        todo!();
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

#[derive(Debug, thiserror::Error)]
pub enum DeployError {
    #[error(transparent)]
    Syscall(#[from] std::io::Error),
}

type Result<T, E = DeployError> = std::result::Result<T, E>;
