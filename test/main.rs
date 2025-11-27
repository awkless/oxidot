// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

mod integration;

use anyhow::Result;
use std::path::Path;
use git2::{IndexEntry, IndexTime, RepositoryInitOptions, Repository};

pub(crate) struct RepoFixture {
    repo: Repository
}

impl RepoFixture {
    pub(crate) fn new(path: impl AsRef<Path>, kind: RepoKind) -> Result<Self> {
        let mut opts = RepositoryInitOptions::new();
        opts.initial_head("main");
        opts.bare(kind.is_bare());
        let repo = Repository::init_opts(path.as_ref(), &opts)?;

        // INVARIANT: Always provide valid name and email.
        //   - Git will complain if this is not set in CI/CD environments.
        let mut config = repo.config()?;
        config.set_str("user.name", "John Doe")?;
        config.set_str("user.email", "john@doe.com")?;

        if kind.is_bare() {
            config.set_str("status.showUntrackedFiles", "no")?;
            config.set_str("core.sparseCheckout", "true")?;
        }

        Ok(Self { repo })
    }

    pub(crate) fn stage_and_commit(
        &self,
        filename: impl AsRef<Path>,
        contents: impl AsRef<str>,
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
            id: self.repo.blob(contents.as_ref().as_bytes())?,
            flags: 0,
            flags_extended: 0,
            path: filename.as_ref().as_os_str().to_string_lossy().into_owned().as_bytes().to_vec(),
        };

        // INVARIANT: Always use new tree produced by index after staging new entry.
        let mut index = self.repo.index()?;
        index.add_frombuffer(&entry, contents.as_ref().as_bytes())?;
        let tree_oid = index.write_tree()?;
        let tree = self.repo.find_tree(tree_oid)?;

        // INVARIANT: Always determine latest parent commits to append to.
        let signature = self.repo.signature()?;
        let mut parents = Vec::new();
        if let Some(parent) = self.repo.head().ok().map(|head| head.target().unwrap()) {
            parents.push(self.repo.find_commit(parent)?);
        }
        let parents = parents.iter().collect::<Vec<_>>();

        // INVARIANT: Commit to HEAD by appending to obtained parent commits.
        self.repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            format!("chore: add {:?}", filename.as_ref()).as_ref(),
            &tree,
            &parents,
        )?;

        Ok(())
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) enum RepoKind {
    #[default]
    Bare,

    Normal,
}

impl RepoKind {
    pub(crate) fn is_bare(&self) -> bool {
        match self {
            Self::Bare => true,
            Self::Normal => false,
        }
    }
}
