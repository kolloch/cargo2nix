//! Resolve dependencies and other data for CrateDerivation.

use anyhow::format_err;
use anyhow::Error;
use cargo_metadata::Node;
use cargo_metadata::Package;
use cargo_metadata::PackageId;
use cargo_metadata::{Dependency, Source};
use cargo_metadata::{DependencyKind, Target};
use cargo_platform::Platform;
use pathdiff::diff_paths;
use semver::Version;
use serde::Deserialize;
use serde::Serialize;
use serde_json::to_string_pretty;
use std::collections::HashMap;
use std::convert::Into;
use std::path::{Path, PathBuf};

use crate::metadata::IndexedMetadata;
#[cfg(test)]
use crate::test;
use crate::GenerateConfig;
use itertools::Itertools;
use std::{collections::btree_map::BTreeMap, fmt::Display};
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
    /// The crate types of the lib targets of this crate, e.g. "lib", "dylib", "rlib", ...
    pub lib_crate_types: Vec<String>,
    pub dependencies: Vec<ResolvedDependency>,
    pub build_dependencies: Vec<ResolvedDependency>,
    pub dev_dependencies: Vec<ResolvedDependency>,
    /// Feature rules. Which feature (key) enables which other features (values).
    pub features: BTreeMap<String, Vec<String>>,
    /// The resolved features for this crate for a default build as returned by cargo.
    pub resolved_default_features: Vec<String>,
    /// The build target for the custom build script.
    pub build: Option<BuildTarget>,
    /// The build target for the library.
    pub lib: Option<BuildTarget>,
    pub binaries: Vec<BuildTarget>,
    pub proc_macro: bool,
    /// This derivation builds the root crate or a workspace member.
    pub is_root_or_workspace_member: bool,
}

impl CrateDerivation {
    pub fn resolve(
        config: &GenerateConfig,
        crate2nix_json: &crate::config::Config,
        metadata: &IndexedMetadata,
        package: &Package,
    ) -> Result<CrateDerivation, Error> {
        let resolved_dependencies = ResolvedDependencies::new(metadata, package)?;

        let build_dependencies =
            resolved_dependencies.filtered_dependencies(|d| d.kind == DependencyKind::Build);

        let dependencies = resolved_dependencies.filtered_dependencies(|d| {
            d.kind == DependencyKind::Normal || d.kind == DependencyKind::Unknown
        });

        let dev_dependencies =
            resolved_dependencies.filtered_dependencies(|d| d.kind == DependencyKind::Development);

        let is_root_or_workspace_member = metadata
            .root
            .iter()
            .chain(metadata.workspace_members.iter())
            .any(|pkg_id| *pkg_id == package.id);

        let package_path = package.manifest_path.parent().unwrap_or_else(|| {
            panic!(
                "WUUT? No parent directory of manifest at {}?",
                package.manifest_path.as_str()
            )
        });

        // This depends on the non-cananocalized package_path (without symlinks
        // resolved).
        let configured_source = if is_root_or_workspace_member {
            // In the resolved data, we don't have the link to the workspace member
            // name anymore. So we need to extract it from the path.
            let configured_source = package_path
                .file_name()
                .and_then(|file_name| crate2nix_json.sources.get(&*file_name).cloned());

            if !crate2nix_json.sources.is_empty() && configured_source.is_none() {
                eprintln!(
                    "warning: Could not find configured source for workspace member {:?}",
                    package_path
                );
            }

            configured_source
        } else {
            None
        };

        let source = if let Some(configured) = configured_source {
            configured.into()
        } else {
            ResolvedSource::new(&config, &package, &package_path)?
        };

        let package_path = package_path.canonicalize().map_err(|e| {
            format_err!(
                "while canonicalizing crate path path {}: {}",
                package_path.as_str(),
                e
            )
        })?;

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

        let binaries = package
            .targets
            .iter()
            .filter_map(|t| {
                if t.kind.iter().any(|k| k == "bin") {
                    BuildTarget::new(&t, &package_path).ok()
                } else {
                    None
                }
            })
            .collect();

        Ok(CrateDerivation {
            crate_name: package.name.clone(),
            edition: package.edition.clone(),
            authors: package.authors.clone(),
            package_id: package.id.clone(),
            version: package.version.clone(),
            source,
            features: package
                .features
                .iter()
                .map(|(name, feature_list)| (name.clone(), feature_list.clone()))
                .collect(),
            resolved_default_features: metadata
                .nodes_by_id
                .get(&package.id)
                .map(|n| n.features.clone())
                .unwrap_or_else(Vec::new),
            lib_crate_types: package
                .targets
                .iter()
                .filter(|target| target.kind.iter().any(|kind| kind.ends_with("lib")))
                .flat_map(|target| target.crate_types.iter())
                .unique()
                .cloned()
                .collect(),
            dependencies,
            build_dependencies,
            dev_dependencies,
            build,
            lib,
            proc_macro,
            binaries,
            is_root_or_workspace_member,
        })
    }
}

#[test]
pub fn minimal_resolve() {
    use cargo_metadata::{Metadata, Resolve};

    let config = test::generate_config();

    let package = test::package("main", "1.2.3");
    let node = test::node(&package.id.repr);

    let mut resolve: Resolve = test::empty_resolve();
    resolve.root = Some(package.id.clone());
    resolve.nodes = vec![node];

    let mut metadata: Metadata = test::empty_metadata();
    metadata.workspace_members = vec![package.id.clone()];
    metadata.packages = vec![package.clone()];
    metadata.resolve = Some(resolve);

    let indexed = IndexedMetadata::new_from(metadata).unwrap();

    println!("indexed: {:#?}", indexed);

    let root_package = &indexed.root_package().expect("root package");
    let crate_derivation = CrateDerivation::resolve(
        &config,
        &crate::config::Config::default(),
        &indexed,
        root_package,
    )
    .unwrap();

    println!("crate_derivation: {:#?}", crate_derivation);

    assert_eq!(crate_derivation.crate_name, "main");
    assert_eq!(
        crate_derivation.version,
        semver::Version::parse("1.2.3").unwrap()
    );
    assert_eq!(crate_derivation.is_root_or_workspace_member, true);
    let empty: Vec<String> = vec![];
    assert_eq!(crate_derivation.lib_crate_types, empty);

    package.close().unwrap();
}

#[test]
pub fn configured_source_is_used_instead_of_local_directory() {
    use std::convert::TryInto;
    use std::str::FromStr;

    let mut env = test::MetadataEnv::default();
    let config = test::generate_config();

    // crate2nix creates a "virtual" workspace which consists of symlinks to the member sources.
    // The symlinks use the source names and this is how we detect that we use a workspace member
    // source.
    // By simulating this layout, we ensure that we do not canonicalize paths at the "wrong"
    // moment.
    let simulated_store_path = env.temp_dir();
    std::fs::File::create(&simulated_store_path.join("Cargo.toml")).expect("File creation failed");
    let workspace_with_symlink = env.temp_dir();
    std::os::unix::fs::symlink(
        &simulated_store_path,
        workspace_with_symlink.join("some_crate"),
    )
    .expect("could not create symlink");
    let manifest_path = workspace_with_symlink.join("some_crate").join("Cargo.toml");

    let mut main = env.add_package_and_node("main");
    main.update_package(|p| p.manifest_path = manifest_path.try_into().unwrap());
    main.make_root();

    let indexed = env.indexed_metadata();

    let root_package = &indexed.root_package().expect("root package");

    let mut crate2nix_json = crate::config::Config::default();
    let source = crate::config::Source::CratesIo {
        name: "some_crate".to_string(),
        version: semver::Version::from_str("1.2.3").unwrap(),
        sha256: "123".to_string(),
    };
    crate2nix_json.upsert_source(None, source.clone());
    let crate_derivation =
        CrateDerivation::resolve(&config, &crate2nix_json, &indexed, root_package).unwrap();

    println!("crate_derivation: {:#?}", crate_derivation);

    assert!(crate_derivation.is_root_or_workspace_member);
    assert_eq!(
        crate_derivation.source,
        ResolvedSource::CratesIo(CratesIoSource {
            name: "some_crate".to_string(),
            version: semver::Version::from_str("1.2.3").unwrap(),
            sha256: Some("123".to_string()),
        })
    );

    env.close();
}

#[test]
pub fn double_crate_with_rename() {
    let mut env = test::MetadataEnv::default();
    let config = test::generate_config();

    let mut main = env.add_package_and_node("main");
    main.make_root();
    main.add_dependency("futures")
        .version_and_package_id("0.1.0")
        .update_package_dep(|d| d.rename = Some("futures01".to_string()))
        .update_node_dep(|n| n.name = "futures01".to_string());
    main.add_dependency("futures")
        .version_and_package_id("0.3.0")
        .update_package_dep(|d| {
            d.uses_default_features = false;
            d.features = vec!["compat".to_string()];
        });

    let indexed = env.indexed_metadata();

    let root_package = &indexed.root_package().expect("root package");

    let crate_derivation = CrateDerivation::resolve(
        &config,
        &crate::config::Config::default(),
        &indexed,
        root_package,
    )
    .unwrap();

    println!("crate_derivation: {:#?}", crate_derivation);

    assert_eq!(crate_derivation.dependencies.len(), 2);

    env.close();
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
            src_path: target
                .src_path
                .canonicalize()?
                .strip_prefix(&package_path)?
                .to_path_buf(),
        })
    }
}

/// Specifies how to retrieve the source code.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub enum ResolvedSource {
    CratesIo(CratesIoSource),
    Git(GitSource),
    LocalDirectory(LocalDirectorySource),
    Nix(NixSource),
}

impl From<crate::config::Source> for ResolvedSource {
    fn from(source: crate::config::Source) -> Self {
        match source {
            crate::config::Source::Git { url, rev, sha256 } => ResolvedSource::Git(GitSource {
                url,
                rev,
                r#ref: None,
                sha256: Some(sha256),
            }),
            crate::config::Source::CratesIo {
                name,
                version,
                sha256,
            } => ResolvedSource::CratesIo(CratesIoSource {
                name,
                version,
                sha256: Some(sha256),
            }),
            crate::config::Source::Nix { file, attr } => {
                ResolvedSource::Nix(NixSource { file, attr })
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct CratesIoSource {
    pub name: String,
    pub version: Version,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct GitSource {
    #[serde(with = "url_serde")]
    pub url: Url,
    pub rev: String,
    pub r#ref: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct LocalDirectorySource {
    path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct NixSource {
    file: crate::config::NixFile,
    attr: Option<String>,
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
                Ok(ResolvedSource::CratesIo(CratesIoSource {
                    name: package.name.clone(),
                    version: package.version.clone(),
                    sha256: None,
                }))
            }
            Some(source) => {
                ResolvedSource::git_or_local_directory(config, package, &package_path, source)
            }
            None => Ok(ResolvedSource::LocalDirectory(LocalDirectorySource {
                path: ResolvedSource::relative_directory(config, package_path)?,
            })),
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

        let branch = query_pairs
            .find(|(k, _)| k == "branch")
            .map(|(_, v)| v.to_string());
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
        Ok(ResolvedSource::Git(GitSource {
            url,
            rev,
            r#ref: branch,
            sha256: None,
        }))
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
        Ok(ResolvedSource::LocalDirectory(LocalDirectorySource {
            path,
        }))
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
            } else if path.starts_with("../") {
                path
            } else {
                PathBuf::from("./").join(path)
            }
        })
    }

    pub fn sha256(&self) -> Option<&String> {
        match self {
            Self::CratesIo(CratesIoSource { sha256, .. }) | Self::Git(GitSource { sha256, .. }) => {
                sha256.as_ref()
            }
            _ => None,
        }
    }

    pub fn with_sha256(&self, sha256: String) -> Self {
        match self {
            Self::CratesIo(source) => Self::CratesIo(CratesIoSource {
                sha256: Some(sha256),
                ..source.clone()
            }),
            Self::Git(source) => Self::Git(GitSource {
                sha256: Some(sha256),
                ..source.clone()
            }),
            _ => self.clone(),
        }
    }

    pub fn without_sha256(&self) -> Self {
        match self {
            Self::CratesIo(source) => Self::CratesIo(CratesIoSource {
                sha256: None,
                ..source.clone()
            }),
            Self::Git(source) => Self::Git(GitSource {
                sha256: None,
                ..source.clone()
            }),
            _ => self.clone(),
        }
    }
}

impl ToString for ResolvedSource {
    fn to_string(&self) -> String {
        match self {
            Self::CratesIo(source) => source.to_string(),
            Self::Git(source) => source.to_string(),
            Self::LocalDirectory(source) => source.to_string(),
            Self::Nix(source) => source.to_string(),
        }
    }
}

impl CratesIoSource {
    pub fn url(&self) -> String {
        // https://www.pietroalbini.org/blog/downloading-crates-io/
        // Not rate-limited, CDN URL.
        format!(
            "https://static.crates.io/crates/{name}/{name}-{version}.crate",
            name = self.name,
            version = self.version
        )
    }
}

impl Display for CratesIoSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.url())
    }
}

impl Display for GitSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let base = format!("{}#{}", self.url, self.rev);
        if let Some(branch) = self.r#ref.as_ref() {
            write!(f, "{} branch: {}", base, branch)
        } else {
            write!(f, "{}", base)
        }
    }
}

impl Display for LocalDirectorySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path.to_str().unwrap())
    }
}

impl Display for NixSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(attr) = self.attr.as_ref() {
            write!(f, "({}).{}", self.file, attr)
        } else {
            write!(f, "{}", self.file)
        }
    }
}

/// Normalize a package name such as cargo does.
fn normalize_package_name(package_name: &str) -> String {
    package_name.replace('-', "_")
}

#[derive(Debug)]
/// Helper to retrieve the `ResolvedDependency` structs for a package/crate.
///
/// For this, we need to join the information from `Dependency`, which contains
/// the dependency requirements as specified in `Cargo.toml`, and `NodeDep` which
/// contains the resolved package to use. Unfortunately, there is no simply key
/// on which to perform the join in the general case.
struct ResolvedDependencies<'a> {
    package: &'a Package,
    /// Packages references in the NodeDeps of this package.
    resolved_packages_by_crate_name: HashMap<String, Vec<&'a Package>>,
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

        let mut resolved_packages_by_crate_name: HashMap<String, Vec<&'a Package>> = HashMap::new();
        for node_dep in &node.deps {
            let package = metadata.pkgs_by_id.get(&node_dep.pkg).ok_or_else(|| {
                format_err!(
                    "No matching package for dependency with package id {} in {}.\n-- Package\n{}\n-- Node\n{}",
                    node_dep.pkg,
                    package.id,
                    to_string_pretty(&package).unwrap_or_else(|_| "ERROR".to_string()),
                    to_string_pretty(&node).unwrap_or_else(|_| "ERROR".to_string()),
                )
            })?;
            let packages = resolved_packages_by_crate_name
                .entry(normalize_package_name(&package.name))
                .or_default();
            packages.push(package);
        }
        Ok(ResolvedDependencies {
            package,
            resolved_packages_by_crate_name,
        })
    }

    fn filtered_dependencies(
        &self,
        filter: impl Fn(&Dependency) -> bool,
    ) -> Vec<ResolvedDependency> {
        let ResolvedDependencies {
            package,
            resolved_packages_by_crate_name,
        } = self;

        let mut resolved = package
            .dependencies
            .iter()
            .filter(|package_dep| filter(package_dep))
            .flat_map(|package_dep| {
                let name: String = normalize_package_name(&package_dep.name);
                let resolved = resolved_packages_by_crate_name
                    .get(&name)
                    .and_then(|packages| {
                        let exact_match = packages
                            .iter()
                            .find(|p| package_dep.req.matches(&p.version));

                        // Strip prerelease/build info from versions if we
                        // did not find an exact match.
                        //
                        // E.g. "*" does not match a prerelease version in this
                        // library but cargo thinks differently.

                        exact_match.or_else(|| {
                            packages.iter().find(|p| {
                                let without_metadata = {
                                    let mut version = p.version.clone();
                                    version.pre = semver::Prerelease::EMPTY;
                                    version.build = semver::BuildMetadata::EMPTY;
                                    version
                                };
                                package_dep.req.matches(&without_metadata)
                            })
                        })
                    });

                let dep_package = resolved?;

                Some(ResolvedDependency {
                    name: package_dep.name.clone(),
                    rename: package_dep.rename.clone(),
                    package_id: dep_package.id.clone(),
                    target: package_dep.target.clone(),
                    optional: package_dep.optional,
                    uses_default_features: package_dep.uses_default_features,
                    features: package_dep.features.clone(),
                })
            })
            .collect::<Vec<ResolvedDependency>>();

        resolved.sort_by(|d1, d2| d1.package_id.cmp(&d2.package_id));
        resolved
    }
}

/// Converts one type into another by serializing/deserializing it.
///
/// Therefore, the output json of `I` must be deserializable to `O`.
#[allow(unused)]
#[cfg(test)]
fn serialize_deserialize<I: Serialize, O>(input: &I) -> O
where
    for<'d> O: Deserialize<'d>,
{
    let json_string = serde_json::to_string(input).expect("serialize");
    let deserialized: Result<O, _> = serde_json::from_str(&json_string);
    deserialized.expect("deserialize")
}

#[test]
pub fn resolved_dependencies_new_with_double_crate() {
    let mut env = test::MetadataEnv::default();

    let mut main = env.add_package_and_node("main");
    main.make_root();
    main.add_dependency("futures")
        .version_and_package_id("0.1.0")
        .update_package_dep(|d| d.rename = Some("futures01".to_string()))
        .update_node_dep(|n| n.name = "futures01".to_string());
    main.add_dependency("futures")
        .version_and_package_id("0.3.0")
        .update_package_dep(|d| {
            d.uses_default_features = false;
            d.features = vec!["compat".to_string()];
        });

    let indexed = env.indexed_metadata();

    let root_package = &indexed.root_package().expect("root package");
    let resolved_deps = ResolvedDependencies::new(&indexed, root_package).unwrap();

    assert_eq!(
        resolved_deps.resolved_packages_by_crate_name.len(),
        1,
        "unexpected packages_by_crate_name: {}",
        serde_json::to_string_pretty(&resolved_deps.resolved_packages_by_crate_name).unwrap()
    );
    assert!(
        resolved_deps
            .resolved_packages_by_crate_name
            .contains_key("futures"),
        "unexpected packages_by_crate_name: {}",
        serde_json::to_string_pretty(&resolved_deps.resolved_packages_by_crate_name).unwrap()
    );
    assert_eq!(
        resolved_deps
            .resolved_packages_by_crate_name
            .get("futures")
            .unwrap()
            .len(),
        2,
        "unexpected packages_by_crate_name: {}",
        serde_json::to_string_pretty(&resolved_deps.resolved_packages_by_crate_name).unwrap()
    );

    env.close();
}

#[test]
pub fn resolved_dependencies_filtered_dependencies_with_double_crate() {
    let mut env = test::MetadataEnv::default();

    let mut main = env.add_package_and_node("main");
    main.make_root();
    main.add_dependency("futures")
        .version_and_package_id("0.1.0")
        .update_package_dep(|d| d.rename = Some("futures01".to_string()))
        .update_node_dep(|n| n.name = "futures01".to_string());
    main.add_dependency("futures")
        .version_and_package_id("0.3.0")
        .update_package_dep(|d| {
            d.uses_default_features = false;
            d.features = vec!["compat".to_string()];
        });

    let indexed = env.indexed_metadata();

    let root_package = &indexed.root_package().expect("root package");
    let resolved_deps = ResolvedDependencies::new(&indexed, root_package).unwrap();

    let filtered_deps = resolved_deps.filtered_dependencies(|d| {
        d.kind == DependencyKind::Normal || d.kind == DependencyKind::Unknown
    });

    assert_eq!(
        filtered_deps.len(),
        2,
        "unexpected resolved dependencies: {}",
        serde_json::to_string_pretty(&filtered_deps).unwrap()
    );

    env.close();
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ResolvedDependency {
    pub name: String,
    /// New name for the dependency if it is renamed.
    pub rename: Option<String>,
    pub package_id: PackageId,
    /// The cfg expression for conditionally enabling the dependency (if any).
    /// Can also be a target "triplet".
    pub target: Option<Platform>,
    /// Whether this dependency is optional and thus needs to be enabled via a feature.
    pub optional: bool,
    /// Whether the crate uses this dependency with default features enabled.
    pub uses_default_features: bool,
    /// Extra-enabled features.
    pub features: Vec<String>,
}
