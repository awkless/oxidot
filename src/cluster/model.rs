// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

use crate::{config::ClusterDefinition, cluster::sparse::SparsityDrafter};

pub struct Cluster {
    definition: ClusterDefinition,
    sparsity: SparsityDrafter,
}

impl Cluster {}
