// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

use anyhow::{anyhow, Result};
use auth_git2::{GitAuthenticator, Prompter};
use git2::Repository;
use indicatif::{MultiProgress, ProgressBar};
use inquire::{Password, Text};
use std::{ffi::OsStr, path::Path, process::Command};

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
    authenticator: GitAuthenticator,
}

impl Cluster {
    pub fn try_new_init(path: impl AsRef<Path>) -> Result<Self> {
        todo!();
    }

    pub fn try_new_open(path: impl AsRef<Path>) -> Result<Self> {
        todo!();
    }

    pub fn try_new_clone(
        url: impl AsRef<str>,
        path: impl AsRef<Path>,
        bar_kind: ProgressBarKind,
    ) -> Result<Self> {
        todo!();
    }
}

#[derive(Clone)]
struct ProgressBarAuthenticator {
    kind: ProgressBarKind,
}

impl ProgressBarAuthenticator {
    fn new(kind: ProgressBarKind) -> Self {
        Self { kind }
    }
}

impl Prompter for ProgressBarAuthenticator {
    fn prompt_username_password(
        &mut self,
        url: &str,
        _config: &git2::Config,
    ) -> Option<(String, String)> {
        let prompt = || -> Option<(String, String)> {
            let username = Text::new("username").prompt().unwrap();
            let password = Password::new("password")
                .without_confirmation()
                .prompt()
                .unwrap();
            Some((username, password))
        };

        match &self.kind {
            ProgressBarKind::SingleBar(bar) => bar.suspend(prompt),
            ProgressBarKind::MultiBar(bar) => bar.suspend(prompt),
        }
    }

    fn prompt_password(
        &mut self,
        username: &str,
        url: &str,
        _config: &git2::Config,
    ) -> Option<String> {
        let prompt = || -> Option<String> {
            let password = Password::new("password")
                .without_confirmation()
                .prompt()
                .unwrap();
            Some(password)
        };

        match &self.kind {
            ProgressBarKind::SingleBar(bar) => bar.suspend(prompt),
            ProgressBarKind::MultiBar(bar) => bar.suspend(prompt),
        }
    }

    fn prompt_ssh_key_passphrase(
        &mut self,
        private_key_path: &Path,
        _git_config: &git2::Config,
    ) -> Option<String> {
        let prompt = || -> Option<String> {
            let password = Password::new("password")
                .without_confirmation()
                .prompt()
                .unwrap();
            Some(password)
        };

        match &self.kind {
            ProgressBarKind::MultiBar(bar) => bar.suspend(prompt),
            ProgressBarKind::SingleBar(bar) => bar.suspend(prompt),
        }
    }
}

#[derive(Clone)]
enum ProgressBarKind {
    SingleBar(ProgressBar),
    MultiBar(MultiProgress),
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
