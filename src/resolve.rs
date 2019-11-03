//! Resolve dependencies and other data for CrateDerivation.

use cargo_metadata::Node;
use cargo_metadata::Package;
use cargo_metadata::PackageId;
use cargo_metadata::{Dependency, Source};
use cargo_metadata::{DependencyKind, Target};
use failure::format_err;
use failure::Error;
use pathdiff::diff_paths;
use semver::Version;
use serde_derive::Deserialize;
use serde_derive::Serialize;
use serde_json::to_string_pretty;
use std::collections::HashMap;
use std::convert::Into;
use std::path::{Path, PathBuf};

use crate::metadata::IndexedMetadata;
use crate::GenerateConfig;
use std::collections::btree_map::BTreeMap;
use url::Url;

/// All data necessary for creating a derivation for a crate.
#[derive(Debug, Deserialize, Serialize)]
pub struct CrateDerivation {
    pub package_id: PackageId,
    pub crate_name: String,
    pub edition: String,
    pub authors: Vec<String>,
    pub version: Version,
    pub source: ResolvedSource,
    pub dependencies: Vec<ResolvedDependency>,
    pub build_dependencies: Vec<ResolvedDependency>,
    /// Feature rules. Which feature (key) enables which other features (values).
    pub features: BTreeMap<String, Vec<String>>,
    /// The resolved features for this crate for a default build as returned by cargo.
    pub resolved_default_features: Vec<String>,
    /// The build target for the custom build script.
    pub build: Option<BuildTarget>,
    /// The build target for the library.
    pub lib: Option<BuildTarget>,
    pub has_bin: bool,
    pub proc_macro: bool,
    // This derivation builds the root crate or a workspace member.
    pub is_root_or_workspace_member: bool,
}

impl CrateDerivation {
    pub fn resolve(
        config: &GenerateConfig,
        metadata: &IndexedMetadata,
        package: &Package,
    ) -> Result<CrateDerivation, Error> {
        let resolved_dependencies = ResolvedDependencies::new(metadata, package)?;

        let build_dependencies =
            resolved_dependencies.filtered_dependencies(|d| d.kind == DependencyKind::Build);
        let dependencies = resolved_dependencies.filtered_dependencies(|d| {
            d.kind == DependencyKind::Normal || d.kind == DependencyKind::Unknown
        });

        let package_path = package
            .manifest_path
            .parent()
            .expect("WUUT? No parent directory of manifest?")
            .canonicalize()
            .expect("Cannot canonicalize package path");

        let lib = package
            .targets
            .iter()
            .find(|t| t.kind.iter().any(|k| k == "lib" || k == "proc-macro"))
            .and_then(|target| BuildTarget::new(&target, &package_path).ok());

        let build = package
            .targets
            .iter()
            .find(|t| t.kind.iter().any(|k| k == "custom-build"))
            .and_then(|target| BuildTarget::new(&target, &package_path).ok());

        let proc_macro = package
            .targets
            .iter()
            .any(|t| t.kind.iter().any(|k| k == "proc-macro"));

        let has_bin = package
            .targets
            .iter()
            .any(|t| t.kind.iter().any(|k| k == "bin"));

        let is_root_or_workspace_member = metadata
            .root
            .iter()
            .chain(metadata.workspace_members.iter())
            .any(|pkg_id| *pkg_id == package.id);

        Ok(CrateDerivation {
            crate_name: package.name.clone(),
            edition: package.edition.clone(),
            authors: package.authors.clone(),
            package_id: package.id.clone(),
            version: package.version.clone(),
            source: ResolvedSource::new(&config, &package, &package_path)?,
            features: package
                .features
                .iter()
                .map(|(name, feature_list)| (name.clone(), feature_list.clone()))
                .collect(),
            resolved_default_features: metadata
                .nodes_by_id
                .get(&package.id)
                .map(|n| n.features.clone())
                .unwrap_or_else(|| Vec::new()),
            dependencies,
            build_dependencies,
            build,
            lib,
            proc_macro,
            has_bin,
            is_root_or_workspace_member,
        })
    }
}

/// A build target of a crate.
#[derive(Debug, Deserialize, Serialize)]
pub struct BuildTarget {
    /// The name of the build target.
    pub name: String,
    /// The relative path of the target source file.
    pub src_path: PathBuf,
}

impl BuildTarget {
    pub fn new(target: &Target, package_path: impl AsRef<Path>) -> Result<BuildTarget, Error> {
        Ok(BuildTarget {
            name: target.name.clone(),
            src_path: target.src_path.strip_prefix(&package_path)?.to_path_buf(),
        })
    }
}

/// Specifies how to retrieve the source code.
#[derive(Debug, Deserialize, Serialize)]
pub enum ResolvedSource {
    CratesIo {
        sha256: Option<String>,
    },
    Git {
        #[serde(with = "url_serde")]
        url: Url,
        rev: String,
        r#ref: Option<String>
    },
    LocalDirectory {
        path: PathBuf,
    },
}

const GIT_SOURCE_PREFIX: &str = "git+";

impl ResolvedSource {
    pub fn new(
        config: &GenerateConfig,
        package: &Package,
        package_path: impl AsRef<Path>,
    ) -> Result<ResolvedSource, Error> {
        match package.source.as_ref() {
            Some(source) if source.is_crates_io() => {
                // Will sha256 will be filled later by prefetch_and_fill_crates_sha256.
                Ok(ResolvedSource::CratesIo { sha256: None })
            }
            Some(source) => {
                ResolvedSource::git_or_local_directory(config, package, &package_path, source)
            }
            None => Ok(ResolvedSource::LocalDirectory {
                path: ResolvedSource::relative_directory(config, package_path)?,
            }),
        }
    }

    fn git_or_local_directory(
        config: &GenerateConfig,
        package: &Package,
        package_path: &impl AsRef<Path>,
        source: &Source,
    ) -> Result<ResolvedSource, Error> {
        let source_string = source.to_string();
        if !source_string.starts_with(GIT_SOURCE_PREFIX) {
            return ResolvedSource::fallback_to_local_directory(
                config,
                package,
                package_path,
                "No 'git+' prefix found.",
            );
        }
        let mut url = url::Url::parse(&source_string[GIT_SOURCE_PREFIX.len()..])?;
        let mut query_pairs = url.query_pairs();

        let branch = query_pairs.find(|(k, _)| k == "branch").map(|(_, v)| v.to_string());
        let rev = if let Some((_, rev)) = query_pairs.find(|(k, _)| k == "rev") {
            rev.to_string()
        } else if let Some(rev) = url.fragment() {
            rev.to_string()
        } else {
            return ResolvedSource::fallback_to_local_directory(
                config,
                package,
                package_path,
                "No git revision found.",
            );
        };
        url.set_query(None);
        url.set_fragment(None);
        Ok(ResolvedSource::Git {
            url,
            rev,
            r#ref: branch,
        })
    }

    fn fallback_to_local_directory(
        config: &GenerateConfig,
        package: &Package,
        package_path: impl AsRef<Path>,
        warning: &str,
    ) -> Result<ResolvedSource, Error> {
        let path = Self::relative_directory(config, package_path)?;
        eprintln!(
            "WARNING: {} Falling back to local directory for crate {} with source {}: {}",
            warning,
            package.id,
            package
                .source
                .as_ref()
                .map(std::string::ToString::to_string)
                .unwrap_or_else(|| "N/A".to_string()),
            &path.to_string_lossy()
        );
        Ok(ResolvedSource::LocalDirectory { path })
    }

    fn relative_directory(
        config: &GenerateConfig,
        package_path: impl AsRef<Path>,
    ) -> Result<PathBuf, Error> {
        // Use local directory. This is the local cargo crate directory in the worst case.

        let mut output_build_file_directory = config
            .output
            .parent()
            .ok_or_else(|| {
                format_err!(
                    "could not get parent of output file '{}'.",
                    config.output.to_string_lossy()
                )
            })?
            .to_path_buf();

        if output_build_file_directory.is_relative() {
            // Deal with "empty" path. E.g. the parent of "Cargo.nix" is "".
            output_build_file_directory = Path::new(".").join(output_build_file_directory);
        }

        output_build_file_directory = output_build_file_directory.canonicalize().map_err(|e| {
            format_err!(
                "could not canonicalize output file directory '{}': {}",
                output_build_file_directory.to_string_lossy(),
                e
            )
        })?;

        Ok(if package_path.as_ref() == output_build_file_directory {
            "./.".into()
        } else {
            let path = diff_paths(package_path.as_ref(), &output_build_file_directory)
                .unwrap_or_else(|| package_path.as_ref().to_path_buf());
            if path == PathBuf::from("../") {
                path.join(PathBuf::from("."))
            }
            else if path.starts_with("../") {
                path
            } else {
                PathBuf::from("./").join(path)
            }
        })
    }
}

/// The resolved dependencies of one package/crate.
struct ResolvedDependencies<'a> {
    /// The corresponding packages for the dependencies.
    packages: Vec<&'a Package>,
    /// The dependencies of the package/crate.
    dependencies: Vec<&'a Dependency>,
}

impl<'a> ResolvedDependencies<'a> {
    fn new(
        metadata: &'a IndexedMetadata,
        package: &'a Package,
    ) -> Result<ResolvedDependencies<'a>, Error> {
        let node: &Node = metadata.nodes_by_id.get(&package.id).ok_or_else(|| {
            format_err!(
                "Could not find node for {}.\n-- Package\n{}",
                &package.id,
                to_string_pretty(&package).unwrap_or_else(|_| "ERROR".to_string())
            )
        })?;

        let mut packages: Vec<&Package> =
            node
                .deps
                .iter()
                .map(|d| {
                    metadata.pkgs_by_id.get(&d.pkg).ok_or_else(|| {
                        format_err!(
                            "No matching package for dependency with package id {} in {}.\n-- Package\n{}\n-- Node\n{}",
                            d.pkg,
                            package.id,
                            to_string_pretty(&package).unwrap_or_else(|_| "ERROR".to_string()),
                            to_string_pretty(&node).unwrap_or_else(|_| "ERROR".to_string()),
                        )
                    })
                })
                .collect::<Result<_, Error>>()?;
        packages.sort_by(|p1, p2| p1.id.cmp(&p2.id));

        Ok(ResolvedDependencies {
            packages,
            dependencies: package.dependencies.iter().collect(),
        })
    }

    fn filtered_dependencies(
        &self,
        filter: impl Fn(&Dependency) -> bool,
    ) -> Vec<ResolvedDependency> {
        /// Normalize a package name such as cargo does.
        fn normalize_package_name(package_name: &str) -> String {
            package_name.replace('-', "_")
        }


        let mut names: HashMap<String, Vec<&&Dependency>> = HashMap::new();
        for d in self.dependencies.iter().filter(|d| filter(d)) {
            let normalized_name = normalize_package_name(&d.name);
            names.entry(normalized_name)
                .and_modify(|ds| ds.push(&d))
                .or_insert_with(|| vec![&d]);
        }

        self.packages
            .iter()
            .flat_map(|d| {
                let normalized_package_name = normalize_package_name(&d.name);
                names
                    .get(&normalized_package_name)
                    .map(|ds| {
                        let dependency = ds[0];
                        let targets = ds.iter()
                                .filter(|d| d.target.is_some())
                                .map(|d| d.target.as_ref().unwrap().to_string())
                                .collect::<Vec<String>>();
                        ResolvedDependency {
                            name: dependency.name.clone(),
                            rename: dependency.rename.clone(),
                            package_id: d.id.clone(),
                            targets,
                            optional: dependency.optional,
                            uses_default_features: dependency.uses_default_features,
                            features: dependency.features.clone(),
                        }
                    })
            })
            .collect()
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ResolvedDependency {
    pub name: String,
    /// New name for the dependency if it is renamed.
    pub rename: Option<String>,
    pub package_id: PackageId,
    /// The cfg expressions for conditionally enabling the dependency (if any).
    /// Can also be a target "triplet".
    pub targets: Vec<String>,
    /// Whether this dependency is optional and thus needs to be enabled via a feature.
    pub optional: bool,
    /// Whether the crate uses this dependency with default features enabled.
    pub uses_default_features: bool,
    /// Extra-enabled features.
    pub features: Vec<String>,
}
