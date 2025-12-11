// SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
// SPDX-License-Identifier: MIT

//! Configuration layout.
//!
//! Specify the layout for configuration files that Oxidot uses to simplify
//! the process of serialization and deserialization. File I/O is left to the
//! caller to figure out.

use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fmt::{Display, Error as FmtError, Formatter, Result as FmtResult},
    path::{Path, PathBuf},
    str::FromStr,
};

/// Cluster definition layout.
///
/// All clusters in oxidot come with a __definition__ file. This file is a
/// simple configuration file that details how the cluster should be
/// configured and managed by not only the cluster itself, but by the cluster
/// store manager as well.
///
/// # General Layout
///
/// A cluster definition is composed of two basic parts: settings and
/// dependencies. The settings section simply defines how the cluster should
/// be configured. The dependencies section lists all dependencies that should
/// be deployed along with the cluster itself. In other words, clusters can
/// list other clusters as dependencies.
#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ClusterDefinition {
    /// Settings for the cluster.
    pub settings: ClusterSettings,

    /// Dependency listing of other clusters.
    #[serde(rename = "dependency")]
    pub dependencies: Option<Vec<ClusterDependency>>,
}

impl FromStr for ClusterDefinition {
    type Err = ConfigError;

    fn from_str(data: &str) -> Result<Self, Self::Err> {
        let mut definition: ClusterDefinition =
            toml::de::from_str(data).map_err(ConfigError::Deserialize)?;

        // INVARIANT: Perform shell expansion on work tree alias field.
        definition.settings.work_tree_alias = WorkTreeAlias::new(
            shellexpand::full(definition.settings.work_tree_alias.to_string().as_str())
                .map_err(ConfigError::ShellExpansion)?
                .into_owned(),
        );

        Ok(definition)
    }
}

impl Display for ClusterDefinition {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> FmtResult {
        fmt.write_str(
            toml::ser::to_string_pretty(self)
                .map_err(ConfigError::Serialize)?
                .as_str(),
        )
    }
}

/// Cluster configuration settings.
///
/// Standard settings to use for any given cluster.
#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ClusterSettings {
    /// Brief description of what the cluster contains.
    pub description: String,

    /// Remove URL to clone cluster from.
    pub url: String,

    /// Work tree alias to use for deployment.
    pub work_tree_alias: WorkTreeAlias,

    /// Default listing of file content to deploy to work tree alias.
    pub include: Option<Vec<String>>,
}

/// Cluster dependency listing.
///
/// List of other clusters to use as dependencies for given cluster.
#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ClusterDependency {
    /// Name of the cluster dependency.
    pub name: String,

    /// Remote URL to clone cluster from if it isn't in the cluster store.
    pub url: String,

    /// Additional listing of file content to deploy.
    pub include: Option<Vec<String>>,
}

/// Path acting as the work tree alias for given cluster.
#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct WorkTreeAlias(PathBuf);

impl WorkTreeAlias {
    /// Construct new work tree alias.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// Convert work tree alias to [`OsString`].
    pub fn to_os_string(&self) -> OsString {
        OsString::from(self.0.to_string_lossy().into_owned())
    }

    /// Treat work tree alias as [`Path`] slice.
    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }
}

impl Display for WorkTreeAlias {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> FmtResult {
        fmt.write_str(self.as_path().to_string_lossy().as_ref())
    }
}

/// Configuration error types.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ConfigError {
    /// Failed to deserialize configuration.
    #[error(transparent)]
    Deserialize(#[from] toml::de::Error),

    /// Failed to serialize configuration.
    #[error(transparent)]
    Serialize(#[from] toml::ser::Error),

    /// Failed to perform shell expansion on configuration.
    #[error(transparent)]
    ShellExpansion(#[from] shellexpand::LookupError<std::env::VarError>),
}

impl From<ConfigError> for FmtError {
    fn from(_: ConfigError) -> Self {
        FmtError
    }
}

/// Friendly result alias :3
type Result<T, E = ConfigError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use pretty_assertions::assert_eq;
    use sealed_test::prelude::*;

    #[sealed_test(env = [("BLAH", "/home/blah/blah")])]
    fn deserialize_cluster_definition() -> anyhow::Result<()> {
        let result: ClusterDefinition = r#"
            [settings]
            description = "blah blah blah"
            url = "https://blah.org/foo.git"
            work_tree_alias = "$BLAH"
            include = ["file1", "file2", "file3"]

            [[dependency]]
            name = "bar"
            url = "https://blah.org/bar.git"
            include = ["file1", "file2", "file3"]
        "#
        .parse()?;

        let expect = ClusterDefinition {
            settings: ClusterSettings {
                description: "blah blah blah".into(),
                url: "https://blah.org/foo.git".into(),
                work_tree_alias: WorkTreeAlias::new("/home/blah/blah"),
                include: Some(vec!["file1".into(), "file2".into(), "file3".into()]),
            },
            dependencies: Some(vec![ClusterDependency {
                name: "bar".into(),
                url: "https://blah.org/bar.git".into(),
                include: Some(vec!["file1".into(), "file2".into(), "file3".into()]),
            }]),
        };

        assert_eq!(result, expect);

        Ok(())
    }

    #[test]
    fn serialize_cluster_definition() {
        let result = ClusterDefinition {
            settings: ClusterSettings {
                description: "blah blah blah".into(),
                url: "https://blah.org/foo.git".into(),
                work_tree_alias: WorkTreeAlias::new("/home/blah/blah"),
                include: Some(vec!["file1".into(), "file2".into(), "file3".into()]),
            },
            dependencies: Some(vec![ClusterDependency {
                name: "bar".into(),
                url: "https://blah.org/bar.git".into(),
                include: Some(vec!["file1".into(), "file2".into(), "file3".into()]),
            }]),
        }
        .to_string();

        let expect = indoc! {r#"
            [settings]
            description = "blah blah blah"
            url = "https://blah.org/foo.git"
            work_tree_alias = "/home/blah/blah"
            include = [
                "file1",
                "file2",
                "file3",
            ]

            [[dependency]]
            name = "bar"
            url = "https://blah.org/bar.git"
            include = [
                "file1",
                "file2",
                "file3",
            ]
        "#};

        assert_eq!(result, expect);
    }
}
