// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-FileCopyrightText: 2024-2025 Eric Urban <hydrogen18@gmail.com>
// SPDX-License-Identifier: MIT

use anyhow::{anyhow, Context, Result};
use auth_git2::{GitAuthenticator, Prompter};
use git2::{
    build::RepoBuilder, Config, FetchOptions, IndexEntry, IndexTime, ObjectType, RemoteCallbacks,
    Repository,
};
use indicatif::{MultiProgress, ProgressBar};
use inquire::{Password, Text};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::{OsStr, OsString},
    fmt::Write as FmtWrite,
    fs::OpenOptions,
    io::{BufRead, BufReader, Read as IoRead, Write as IoWrite},
    path::{Path, PathBuf},
    process::Command,
};
use tracing::{info, instrument, warn};

/// Cluster store management.
///
/// Oxidot keeps track of available clusters through a __cluster store__. The
/// cluster store is just a basic external directory where all of the clusters
/// are kept for easy access.
///
/// # Naming Conventions
///
/// Each entry in the cluster store comes with a ".git" extension. The
/// name of each cluster is just the name of directory stored in the cluster
/// store itself. Thus, a cluster named "editor" will have a corresponding
/// bare-alias repository in the cluster store as "editor.git".
///
/// Oxidot only considers the top-level of the cluster store when processing
/// cluster data. Thus, it is not possible for oxidot to detect nested clusters.
///
/// # Cluster Store Location
///
/// The cluster store can be placed pretty much anywhere the caller wants within
/// their filesystem. At least when it comes to this API. Typically, as a
/// default path, oxidot idiomatically prefers `$XDG_DATA_HOME/oxidot-store`.
/// However, that is definitely a preference, and not a hard coded rule.
///
/// # See Also
///
/// 1. [`Cluster`](struct.Cluster)
pub struct Store {
    store_path: PathBuf,
    clusters: HashMap<String, Cluster>,
}

impl Store {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let store_path = path.into();
        let pattern = store_path.join("*.git").to_string_lossy().into_owned();
        let mut clusters = HashMap::new();
        for entry in glob::glob(pattern.as_str())? {
            // INVARIANT: The name of a cluster is the directory name minus the .git extension.
            let path = entry?;
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            clusters.insert(name, Cluster::try_new_open(path)?);
        }

        // TODO: Add checks for valid cluster store structure at some point.
        let store = Self {
            store_path,
            clusters,
        };
        Ok(store)
    }

    pub fn insert(&mut self, name: impl Into<String>, cluster: Cluster) -> Option<Cluster> {
        self.clusters.insert(name.into(), cluster)
    }

    pub fn get(&self, name: impl AsRef<str>) -> Result<&Cluster> {
        self.clusters
            .get(name.as_ref())
            .ok_or(anyhow!("cluster {:?} not in store", name.as_ref()))
    }

    pub fn resolve_dependencies(&mut self, cluster_name: impl AsRef<str>) -> Result<Vec<String>> {
        let mut resolved = Vec::new();
        let mut visited = HashSet::new();
        let mut stack = VecDeque::new();
        stack.push_back(cluster_name.as_ref().to_string());

        while let Some(current) = stack.pop_back() {
            if !visited.insert(current.clone()) {
                continue;
            }

            if !self.clusters.contains_key(&current) {
                self.clone_missing_cluster(&current)?;
            }

            if let Some(cluster) = self.clusters.get(&current) {
                resolved.push(current.clone());
                if let Some(deps) = &cluster.definition.dependencies {
                    for dep in deps {
                        if !visited.contains(&dep.name) {
                            stack.push_back(dep.name.clone());
                        }
                    }
                }
            }
        }

        Ok(resolved)
    }

    fn clone_missing_cluster(&mut self, name: impl AsRef<str>) -> Result<()> {
        let dep_info = self
            .clusters
            .values()
            .flat_map(|c| c.definition.dependencies.iter().flatten())
            .find(|d| d.name == name.as_ref())
            .ok_or_else(|| anyhow!("dependency {:?} no declared", name.as_ref()))?;

        let path = self.store_path.join(format!("{}.git", name.as_ref()));
        let bars = MultiProgress::new();
        let bar = bars.add(ProgressBar::no_length());
        let auth_bar = ProgressBarAuthenticator::new(bar.clone());

        let cluster = Cluster::try_new_clone(&dep_info.url, path, auth_bar)?;
        bar.finish_and_clear();
        self.clusters.insert(name.as_ref().into(), cluster);

        Ok(())
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
    sparse_checkout: SparseCheckout,
}

impl Cluster {
    #[instrument(skip(path, definition), level = "debug")]
    pub fn try_new_init(path: impl AsRef<Path>, definition: ClusterDefinition) -> Result<Self> {
        info!("initialize new cluster: {:?}", path.as_ref().display());
        let repository = Repository::init_bare(path.as_ref())?;
        let mut config = repository.config()?;
        config.set_str("status.showUntrackedFiles", "no")?;
        config.set_str("core.sparseCheckout", "true")?;
        let sparse_checkout = SparseCheckout::new(path.as_ref())?;

        let cluster = Self {
            repository,
            definition,
            sparse_checkout,
        };

        let contents = toml::ser::to_string_pretty(&cluster.definition)?;
        info!(
            "stage and commit the following cluster definition:\n{}",
            contents
        );
        cluster.stage_and_commit("cluster.toml", contents, "chore: add cluster.toml")?;

        Ok(cluster)
    }

    #[instrument(skip(path), level = "debug")]
    pub fn try_new_open(path: impl AsRef<Path>) -> Result<Self> {
        info!("open cluster: {:?}", path.as_ref().display());
        let repository = Repository::open(path.as_ref())?;
        let mut cluster = Self {
            repository,
            definition: ClusterDefinition::default(),
            sparse_checkout: SparseCheckout::new(path.as_ref())?,
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
            sparse_checkout: SparseCheckout::new(path.as_ref())?,
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

    #[instrument(skip(self, rules), level = "debug")]
    pub fn deploy_rules(&self, rules: impl IntoIterator<Item = impl Into<String>>) -> Result<()> {
        info!("deploy {:?}", self.repository.path().display());
        if self.is_empty() {
            warn!("cluster {:?} is empty", self.repository.path().display());
            return Ok(());
        }

        self.sparse_checkout.insert_rules(rules)?;
        let output = self.gitcall_non_interactive(["checkout"])?;
        info!("{output}");

        Ok(())
    }

    #[instrument(skip(self, rules), level = "debug")]
    pub fn undeploy_rules(&self, rules: impl IntoIterator<Item = impl Into<String>>) -> Result<()> {
        info!("undeploy {:?}", self.repository.path().display());
        if self.is_empty() {
            warn!("cluster {:?} is empty", self.repository.path().display());
            return Ok(());
        }

        self.sparse_checkout.remove_rules(rules)?;
        let output = self.gitcall_non_interactive(["checkout"])?;
        info!("{output}");
        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub fn undeploy_all(&self) -> Result<()> {
        if !self.is_deployed()? {
            warn!(
                "cluster {:?} already undeployed in full",
                self.repository.path().display()
            );
            return Ok(());
        }

        self.sparse_checkout.clear_rules()?;
        let output = self.gitcall_non_interactive(["checkout"])?;
        info!("{output}");

        Ok(())
    }

    pub fn is_deployed(&self) -> Result<bool> {
        if self.is_empty() {
            return Ok(false);
        }

        let entries: Vec<String> = self
            .list_file_paths()?
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();

        for entry in entries {
            let path = self.definition.settings.work_tree_alias.0.join(entry);
            if !path.exists() {
                return Ok(false);
            }
        }

        Ok(true)
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

    // Thank you Eric at https://www.hydrogen18.com/blog/list-all-files-git-repo-pygit2.html.
    fn list_file_paths(&self) -> Result<Vec<PathBuf>> {
        let mut entries = Vec::new();
        let commit = self.repository.head()?.peel_to_commit()?;
        let tree = commit.tree()?;
        let mut trees_and_paths = VecDeque::new();
        trees_and_paths.push_front((tree, PathBuf::new()));

        while let Some((tree, path)) = trees_and_paths.pop_front() {
            for tree_entry in &tree {
                match tree_entry.kind() {
                    Some(ObjectType::Tree) => {
                        let next_tree = self.repository.find_tree(tree_entry.id())?;
                        let next_path = path.join(bytes_to_path(tree_entry.name_bytes()));
                        trees_and_paths.push_front((next_tree, next_path));
                    }
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

#[derive(Debug, Clone)]
pub struct SparseCheckout {
    sparse_path: PathBuf,
}

impl SparseCheckout {
    pub fn new(gitdir: impl Into<PathBuf>) -> Result<Self> {
        let sparse_path = gitdir.into().join("info").join("sparse-checkout");
        let sparse_checkout = Self { sparse_path };
        sparse_checkout.clear_rules()?;

        Ok(sparse_checkout)
    }

    pub fn insert_rules(&self, rules: impl IntoIterator<Item = impl Into<String>>) -> Result<()> {
        let new_rules = rules.into_iter().fold(String::new(), |mut acc, u| {
            writeln!(&mut acc, "{}", u.into()).unwrap();
            acc
        });

        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&self.sparse_path)
            .with_context(|| {
                anyhow!("failed to create or open {:?}", self.sparse_path.display())
            })?;

        let mut rule_set = String::new();
        file.read_to_string(&mut rule_set)
            .with_context(|| anyhow!("failed to read {:?}", self.sparse_path.display()))?;
        rule_set.push_str(new_rules.as_str());

        // INVARIANT: Remove duplicate sparsity rules.
        let mut seen = HashSet::new();
        let rule_set = rule_set
            .lines()
            .filter(|line| seen.insert(*line))
            .collect::<Vec<_>>()
            .join("\n");
        file.write_all(rule_set.as_bytes()).with_context(|| {
            anyhow!(
                "failed to write sparsity rules to {:?}",
                self.sparse_path.display()
            )
        })?;

        Ok(())
    }

    pub fn remove_rules(&self, rules: impl IntoIterator<Item = impl Into<String>>) -> Result<()> {
        let old_rules = rules
            .into_iter()
            .map(|rule| rule.into())
            .collect::<HashSet<_>>();

        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&self.sparse_path)
            .with_context(|| {
                anyhow!("failed to create or open {:?}", self.sparse_path.display())
            })?;
        let reader = BufReader::new(&file);
        let mut rules = reader.lines().into_iter().flatten().collect::<Vec<_>>();
        let _ = rules.extract_if(.., |rule| old_rules.contains(rule));

        file.write_all(
            rules
                .into_iter()
                .fold(String::new(), |mut acc, u| {
                    writeln!(&mut acc, "{u}").unwrap();
                    acc
                })
                .as_bytes(),
        )
        .with_context(|| anyhow!("failed to remove rules in {:?}", self.sparse_path.display()))?;

        Ok(())
    }

    pub fn clear_rules(&self) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&self.sparse_path)
            .with_context(|| {
                anyhow!("failed to create or open {:?}", self.sparse_path.display())
            })?;
        file.write_all(b"").with_context(|| {
            anyhow!("failed to clear rules in {:?}", self.sparse_path.display())
        })?;

        Ok(())
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
