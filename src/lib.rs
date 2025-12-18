// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

//! Oxidot's internal library.
//!
//! Helpful internal library used to implement the core logic used by the Oxidot
//! binary. Generally only meant to be used for Oxidot only.
//!
//! # Introduction
//!
//! Oxidot is an experimental dotfile management tool that operates through
//! __clusters__. A cluster is a bare-alias repository whose contents can be
//! deployed to a target working tree alias. Oxidot allows the user to create
//! and manage multiple clusters through a __cluster store__. The cluster store
//! is the area where each cluster is actually housed. By default the cluster
//! store can be found at `$XDG_DATA_HOME/oxidot-store`.
//!
//! Through Oxidot the user can treat a given directory like a Git repository
//! without needing to initialize it as one for multiple configurations across
//! multiple clusters. Configurations can be deployed or undeployed easily,
//! without the need to copy, move, or symlink them from any given cluster
//! in the cluster store.
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
//! # Deployment
//!
//! Oxidot allows the user to selectively deploy the contents of a cluster to
//! its target work tree alias via sparse checkout. Thus, the user must supply
//! a listing of sparsity rules that match the file content they want deployed
//! to a cluster's work tree alias. By default, no component of the cluster is
//! deployed, unless the user specifies a default set of deployment rules to
//! use.
//!
//! > __NOTE__: Oxidot typically uses the terms _sparsity rules_ and
//! > _deployment rules_ interchangeably.
//!
//! # See Also
//!
//! 1. [ArchWiki - dotfiles](https://wiki.archlinux.org/title/Dotfiles#Tracking_dotfiles_directly_with_Git)

#![warn(
    clippy::complexity,
    clippy::correctness,
    missing_debug_implementations,
    rust_2021_compatibility
)]
#![doc(issue_tracker_base_url = "https://github.com/awkless/oxidot/issues")]

pub mod cluster;
pub mod config;
pub mod path;
pub mod store;
