// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

use anyhow::{anyhow, Result};
use auth_git2::{GitAuthenticator, Prompter};
use git2::{build::RepoBuilder, Config, FetchOptions, RemoteCallbacks, Repository};
use indicatif::ProgressBar;
use inquire::{Password, Text};
use serde::Deserialize;
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};
use tracing::{info, instrument};

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
    repository: Repository,
    definition: ClusterDefinition,
}

impl Cluster {
    pub fn try_new_init(path: impl AsRef<Path>) -> Result<Self> {
        let repository = Repository::init_bare(path)?;
        let mut config = repository.config()?;
        config.set_str("status.showUntrackedFiles", "no")?;
        config.set_str("core.sparseCheckout", "true")?;

        let mut definition = ClusterDefinition::default();
        definition.settings.worktree_alias = WorkTreeAlias::try_default()?;

        Ok(Self {
            repository,
            definition,
        })
    }

    pub fn try_new_open(path: impl AsRef<Path>) -> Result<Self> {
        let repository = Repository::open(path)?;
        let mut config = repository.config()?;

        if config.get_str("status.showUntrackedFiles")? != "no" {
            config.set_str("status.showUntrackedFiles", "no")?;
        }

        if config.get_str("core.sparseCheckout")? != "true" {
            config.set_str("core.sparseCheckout", "true")?;
        }

        let mut cluster = Self {
            repository,
            definition: ClusterDefinition::default(),
        };
        cluster.extract_cluster_definition()?;

        Ok(cluster)
    }

    pub fn try_new_clone(
        url: impl AsRef<str>,
        path: impl AsRef<Path>,
        prompter: ProgressBarAuthenticator,
    ) -> Result<Self> {
        let authenticator = GitAuthenticator::default().set_prompter(prompter.clone());
        let config = Config::open_default()?;
        let mut rc = RemoteCallbacks::new();
        rc.credentials(authenticator.credentials(&config));
        rc.transfer_progress(|progress| {
            let stats = progress.to_owned();
            let bar_size = stats.total_objects() as u64;
            let bar_pos = stats.received_objects() as u64;
            prompter.bar.set_length(bar_size);
            prompter.bar.set_position(bar_pos);
            true
        });

        let mut fo = FetchOptions::new();
        fo.remote_callbacks(rc);

        let repository = RepoBuilder::new()
            .bare(true)
            .fetch_options(fo)
            .clone(url.as_ref(), path.as_ref())?;

        let mut cluster = Self {
            repository,
            definition: ClusterDefinition::default(),
        };
        cluster.extract_cluster_definition()?;

        Ok(cluster)
    }

    pub fn gitcall_non_interactive(
        &self,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<String> {
        todo!();
    }

    fn extract_cluster_definition(&mut self) -> Result<()> {
        let commit = self.repository.head()?.peel_to_commit()?;
        let tree = commit.tree()?;
        let blob = tree
            .get_name("cluster.toml")
            .map(|entry| entry.to_object(&self.repository)?.peel_to_blob())
            .ok_or(anyhow!("cluster has no definition file"))??;
        let content = String::from_utf8_lossy(blob.content()).into_owned();
        self.definition = toml::de::from_str::<ClusterDefinition>(&content)?;

        Ok(())
    }

    fn expand_bin_args(
        &self,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Vec<OsString> {
        todo!();
    }
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize)]
pub struct ClusterDefinition {
    pub settings: ClusterSettings,
    pub dependencies: Option<Vec<ClusterDependency>>,
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize)]
pub struct ClusterSettings {
    description: String,
    url: String,
    worktree_alias: WorkTreeAlias,
    include: Option<Vec<String>>,
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize)]
pub struct ClusterDependency {
    name: String,
    url: String,
    include: Option<Vec<String>>,
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize)]
pub struct WorkTreeAlias(pub PathBuf);

impl WorkTreeAlias {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn try_default() -> Result<Self> {
        Ok(Self(home_dir()?))
    }

    pub fn to_os_string(&self) -> OsString {
        OsString::from(self.0.to_string_lossy().into_owned())
    }
}

#[derive(Clone)]
pub struct ProgressBarAuthenticator {
    pub(crate) bar: ProgressBar,
}

impl ProgressBarAuthenticator {
    pub fn new(bar: ProgressBar) -> Self {
        Self { bar }
    }
}

impl Prompter for ProgressBarAuthenticator {
    #[instrument(skip(self, url, _config), level = "debug")]
    fn prompt_username_password(
        &mut self,
        url: &str,
        _config: &git2::Config,
    ) -> Option<(String, String)> {
        info!("authentication required at {url}");
        self.bar.suspend(|| -> Option<(String, String)> {
            let username = Text::new("username").prompt().unwrap();
            let password = Password::new("password")
                .without_confirmation()
                .prompt()
                .unwrap();
            Some((username, password))
        })
    }

    #[instrument(skip(self, username, url, _config), level = "debug")]
    fn prompt_password(
        &mut self,
        username: &str,
        url: &str,
        _config: &git2::Config,
    ) -> Option<String> {
        info!("authentication required at {url} for user {username}");
        self.bar.suspend(|| -> Option<String> {
            let password = Password::new("password")
                .without_confirmation()
                .prompt()
                .unwrap();
            Some(password)
        })
    }

    #[instrument(skip(self, ssh_key_path, _config), level = "debug")]
    fn prompt_ssh_key_passphrase(
        &mut self,
        ssh_key_path: &Path,
        _config: &git2::Config,
    ) -> Option<String> {
        info!(
            "authentication required with ssh key at {}",
            ssh_key_path.display()
        );
        self.bar.suspend(|| -> Option<String> {
            let password = Password::new("password")
                .without_confirmation()
                .prompt()
                .unwrap();
            Some(password)
        })
    }
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
        message.push_str(format!("stderr: {stderr}").as_str());
    }

    if !output.status.success() {
        return Err(anyhow!("command {:?} failed:\n{message}", cmd.as_ref()));
    }

    // INVARIANT: Chomp trailing newlines.
    let message = message
        .strip_suffix("\r\n")
        .or(message.strip_suffix('\n'))
        .map(ToString::to_string)
        .unwrap_or(message);

    Ok(message)
}

fn syscall_interactive(
    cmd: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> Result<()> {
    let status = Command::new(cmd.as_ref()).args(args).spawn()?.wait()?;
    if !status.success() {
        return Err(anyhow!("command {:?} failed", cmd.as_ref()));
    }

    Ok(())
}

fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or(anyhow!("cannot determine path to home directory"))
}
