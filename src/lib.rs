// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

use anyhow::{anyhow, Result};
use auth_git2::{GitAuthenticator, Prompter};
use git2::{
    build::RepoBuilder, Config, FetchOptions, IndexEntry, IndexTime, RemoteCallbacks, Repository,
};
use indicatif::ProgressBar;
use inquire::{Password, Text};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};
use tracing::{info, instrument};

pub struct Store {
    clusters: HashMap<String, Cluster>,
}

impl Store {
    pub fn new(cluster_store: impl Into<PathBuf>) -> Result<Self> {
        let pattern = cluster_store
            .into()
            .join("*.git")
            .to_string_lossy()
            .into_owned();
        let mut clusters = HashMap::new();
        for entry in glob::glob(pattern.as_str())? {
            // INVARIANT: The name of a cluster is the directory name minus the .git extension.
            let path = entry?;
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            clusters.insert(name, Cluster::try_new_open(path)?);
        }

        Ok(Self { clusters })
    }
}

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
    #[instrument(skip(path, definition), level = "debug")]
    pub fn try_new_init(path: impl AsRef<Path>, definition: ClusterDefinition) -> Result<Self> {
        info!("initialize new cluster: {:?}", path.as_ref().display());
        let repository = Repository::init_bare(path)?;
        let mut config = repository.config()?;
        config.set_str("status.showUntrackedFiles", "no")?;
        config.set_str("core.sparseCheckout", "true")?;

        let cluster = Self {
            repository,
            definition,
        };

        let contents = toml::ser::to_string_pretty(&cluster.definition)?;
        info!(
            "stage and commit the following cluster definition:\n{}",
            contents
        );
        cluster.stage_and_commit("cluster.toml", contents, format!("chore: add cluster.toml"))?;

        Ok(cluster)
    }

    #[instrument(skip(path), level = "debug")]
    pub fn try_new_open(path: impl AsRef<Path>) -> Result<Self> {
        info!("open cluster: {:?}", path.as_ref().display());
        let repository = Repository::open(path)?;
        let mut cluster = Self {
            repository,
            definition: ClusterDefinition::default(),
        };
        cluster.extract_cluster_definition()?;

        let mut config = cluster.repository.config()?;
        if cluster.get_config_value(&config, "status.showUntrackedFiles")? != Some("no".into()) {
            config.set_str("status.showUntrackedFiles", "no")?;
        }

        if cluster.get_config_value(&config, "core.sparseCheckout")? != Some("true".into()) {
            config.set_str("core.sparseCheckout", "true")?;
        }

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

    pub fn stage_and_commit(
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

    pub fn is_empty(&self) -> bool {
        self.repository
            .head()
            .ok()
            .and_then(|head| head.target())
            .and_then(|oid| self.repository.find_commit(oid).ok())
            .is_none()
    }

    pub fn gitcall_non_interactive(
        &self,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<String> {
        syscall_non_interactive("git", self.expand_bin_args(args))
    }

    pub fn gitcall_interactive(
        &self,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<()> {
        syscall_interactive("git", self.expand_bin_args(args))
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
        let gitdir = self.repository.path().to_string_lossy().into_owned().into();
        let path_args: Vec<OsString> = vec![
            "--git-dir".into(),
            gitdir,
            "--work-tree".into(),
            self.definition.settings.work_tree_alias.to_os_string(),
        ];

        let mut bin_args: Vec<OsString> = Vec::new();
        bin_args.extend(path_args);
        bin_args.extend(args.into_iter().map(Into::into));

        bin_args
    }

    fn get_config_value(&self, config: &git2::Config, key: &str) -> Result<Option<String>> {
        match config.get_entry(key) {
            Ok(entry) => Ok(entry.value().map(|v| v.to_string())),
            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(err) => Err(anyhow!(err)),
        }
    }
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ClusterDefinition {
    pub settings: ClusterSettings,
    pub dependencies: Option<Vec<ClusterDependency>>,
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ClusterSettings {
    pub description: String,
    pub url: String,
    pub work_tree_alias: WorkTreeAlias,
    pub include: Option<Vec<String>>,
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ClusterDependency {
    pub name: String,
    pub url: String,
    pub include: Option<Vec<String>>,
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
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

pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or(anyhow!("cannot determine path to home directory"))
}

pub fn cluster_store_dir() -> Result<PathBuf> {
    dirs::data_dir()
        .map(|path| path.join("oxidot-store"))
        .ok_or(anyhow!("cannot determine path to cluster store"))
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
