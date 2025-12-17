// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-FileCopyrightText: 2024-2025 Eric Urban <hydrogen18@gmail.com>
// SPDX-License-Identifier: MIT

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
