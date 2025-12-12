// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

use crate::{
    cluster::sparse::{InvertedGitignore, SparsityDrafter},
    config::ClusterDefinition,
};

#[derive(Debug)]
pub struct Cluster {
    pub definition: ClusterDefinition,
    pub sparsity: SparsityDrafter<InvertedGitignore>,
}

impl Cluster {
    pub fn new(
        definition: ClusterDefinition,
        sparsity: SparsityDrafter<InvertedGitignore>,
    ) -> Self {
        Self {
            definition,
            sparsity,
        }
    }
}
