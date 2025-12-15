// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

//! Cluster domain representation.
//!
//! A __cluster__ is a bare-alias repository whose contents can be deployed to a
//! target working tree alias.
//!
//! # Bare-Alias Repositories
//!
//! All clusters in oxidot are considered __bare-alias__ repositories. Although
//! bare repositories lack a working tree by definition, Git allows users to
//! force a working tree by designating a directory as an alias for a working
//! tree using the "--work-tree" argument. This functionality enables us to
//! define a bare repository where the Git directory, and the alias working tree
//! are kept separate. This unique feature allows us to treat an entire
//! directory as a Git repository without needing to initialize it as one.
//!
//! This technique does not really have a standard name despite being a common
//! method to manage dotfile configurations through Git. Se we call it the
//! __bare-alias technique__. Hence, the term _bare-alias_ repository!
//!
//! # Cluster Components
//!
//! A cluster mainly contains two basic things: tracked files, and a
//! __cluster definition__. Tracked files are the various dotfile configurations
//! that the cluster needs to keep track of, and deploy to its target work tree
//! alias. However, the cluster definition specifies the actual configuration
//! settings and dependencies of the cluster itself.
//!
//! ## Cluster Definition
//!
//! A cluster definition is a special tracked file that specifies configuration
//! settings that are needed to determine how Oxidot should treat a given
//! cluster, e.g., give basic description of the cluster, specify the work tree
//! alias to use, etc.  The cluster definition can also be used to list other
//! clusters as dependencies of the current cluster. These dependencies will be
//! deployed along side their parent cluster.
//!
//! All clusters must contain a valid definition file at the top-level named
//! "cluster.toml". If this file cannot be found, then the cluster is considered
//! to be invalid, i.e., not a true cluster. Thus, all clusters must be
//! bare-alias and contain a cluster definition file to be considered a valid
//! cluster.
//!
//! # Cluster Deployment
//!
//! Oxidot performs cluster deployment through Git's sparse checkout feature.
//! The user must supply a valid listing of spasrity rules that match the
//! tracked files that they want deployed to a any given cluster's work tree
//! alias. Sparse checkout allows Oxidot's cluster deployment feature to
//! properly deploy tracked files without touching the commit history or
//! index of the cluster itself. This also simplfies deployment logic, because
//! a good portion of it is offloaded to Git.
//!
//! # See Also
//!
//! 1. [ArchWiki - dotfiles](https://wiki.archlinux.org/title/Dotfiles#Tracking_dotfiles_directly_with_Git)
//! 2. [`ClusterDefinition`](crate::config::ClusterDefinition)
//! 3. [`sparse`](crate::cluster::sparse)

use crate::{
    cluster::deploy::{Deployment, Git2Deployer},
    config::ClusterDefinition,
};

use std::{ffi::OsString, path::Path};

/// A basic cluster.
///
/// A __cluster__ is a bare-alias repository whose contents can be deployed to a
/// target working tree alias. Through a cluster, the user can keep track of
/// essential files in a target directory labeled as a work tree alias, without
/// needing to initialize it as a Git repository. Tracked files can be deployed
/// or undeployed to the work tree alias at will.
#[derive(Debug)]
pub struct Cluster<D = Git2Deployer>
where
    D: Deployment,
{
    pub(crate) definition: ClusterDefinition,
    deployer: D,
}

impl<D> Cluster<D>
where
    D: Deployment,
{
    /// Construct new cluster.
    pub fn new(definition: ClusterDefinition, deployer: D) -> Self {
        Self {
            definition,
            deployer,
        }
    }

    pub fn deploy_with_rules(
        &self,
        rules: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<()> {
        Ok(self
            .deployer
            .deploy_with_rules(&self.definition.settings.work_tree_alias, rules)?)
    }

    pub fn undeploy_with_rules(
        &self,
        rules: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<()> {
        Ok(self
            .deployer
            .undeploy_with_rules(&self.definition.settings.work_tree_alias, rules)?)
    }

    pub fn deploy_default_rules(&self) -> Result<()> {
        if let Some(default) = &self.definition.settings.include {
            self.deployer
                .deploy_with_rules(&self.definition.settings.work_tree_alias, default)?;
        }

        Ok(())
    }

    pub fn undeploy_default_rules(&self) -> Result<()> {
        if let Some(default) = &self.definition.settings.include {
            self.deployer
                .undeploy_with_rules(&self.definition.settings.work_tree_alias, default)?;
        }

        Ok(())
    }

    pub fn deploy_all(&self) -> Result<()> {
        Ok(self
            .deployer
            .deploy_all(&self.definition.settings.work_tree_alias)?)
    }

    pub fn undeploy_all(&self) -> Result<()> {
        Ok(self
            .deployer
            .undeploy_all(&self.definition.settings.work_tree_alias)?)
    }

    pub fn is_deployed(&self) -> bool {
        self.deployer
            .is_deployed(&self.definition.settings.work_tree_alias)
    }

    pub fn gitcall_interactive(
        &self,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<()> {
        Ok(self
            .deployer
            .gitcall_interactive(&self.definition.settings.work_tree_alias, args)?)
    }

    pub fn gitcall_non_interactive(
        &self,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<String> {
        Ok(self
            .deployer
            .gitcall_non_interactive(&self.definition.settings.work_tree_alias, args)?)
    }
}

pub trait ClusterAccess {
    fn try_init(path: impl AsRef<Path>, definition: ClusterDefinition) -> Result<Cluster>;
    fn try_open(path: impl AsRef<Path>) -> Result<Cluster>;
    fn try_clone(url: impl AsRef<str>, path: impl AsRef<Path>) -> Result<Cluster>;
}

pub struct Git2Cluster;

impl ClusterAccess for Git2Cluster {
    fn try_init(path: impl AsRef<Path>, definition: ClusterDefinition) -> Result<Cluster> {
        todo!();
    }

    fn try_open(path: impl AsRef<Path>) -> Result<Cluster> {
        todo!();
    }

    fn try_clone(url: impl AsRef<str>, path: impl AsRef<Path>) -> Result<Cluster> {
        todo!();
    }
}


#[derive(Debug, thiserror::Error)]
pub enum ClusterError {
    #[error(transparent)]
    Deployment(#[from] crate::cluster::deploy::DeployError),
}

type Result<T, E = ClusterError> = std::result::Result<T, E>;
