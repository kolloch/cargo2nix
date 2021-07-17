//! # crate2nix
//!
//! Internal library for the crate2nix binary. This is not meant to be used separately, I just enjoy
//! writing doc tests ;)
//!
//! [Repository](https://github.com/kolloch/crate2nix)

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::env;
use std::path::PathBuf;
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::Path,
};

use anyhow::format_err;
use anyhow::Context;
use anyhow::Error;
use cargo_metadata::Metadata;
use cargo_metadata::PackageId;
use serde::Deserialize;
use serde::Serialize;

use crate::metadata::IndexedMetadata;
use crate::resolve::{CrateDerivation, ResolvedSource};
use itertools::Itertools;
use resolve::CratesIoSource;

mod command;
pub mod config;
mod lock;
mod metadata;
pub mod nix_build;
mod prefetch;
pub mod render;
mod resolve;
pub mod sources;
#[cfg(test)]
pub mod test;
pub mod util;

/// The resolved build info and the input for rendering the build.nix.tera template.
#[derive(Debug, Deserialize, Serialize)]
pub struct BuildInfo {
    /// The package ID of the root crate.
    pub root_package_id: Option<PackageId>,
    /// Workspaces member package IDs by package names.
    pub workspace_members: BTreeMap<String, PackageId>,
    /// Build info for all crates needed for this build.
    pub crates: Vec<CrateDerivation>,
    /// For convenience include the source for tests.
    pub indexed_metadata: IndexedMetadata,
    /// The generation configuration.
    pub info: GenerateInfo,
    /// The generation configuration.
    pub config: GenerateConfig,
}

impl BuildInfo {
    /// Return the `NixBuildInfo` data ready for rendering the nix build file.
    pub fn for_config(info: &GenerateInfo, config: &GenerateConfig) -> Result<BuildInfo, Error> {
        let merged = {
            let mut metadatas = Vec::new();
            for cargo_toml in &config.cargo_toml {
                metadatas.push(cargo_metadata(config, cargo_toml)?);
            }
            metadata::MergedMetadata::merge(metadatas)?
        };

        let indexed_metadata = IndexedMetadata::new_from_merged(merged).map_err(|e| {
            format_err!(
                "while indexing metadata for {:#?}: {}",
                config
                    .cargo_toml
                    .iter()
                    .map(|p| p.to_string_lossy())
                    .collect::<Vec<_>>(),
                e
            )
        })?;
        let mut default_nix = BuildInfo::new(info, config, indexed_metadata)?;

        default_nix.prune_unneeded_crates();

        prefetch_and_fill_crates_sha256(config, &mut default_nix)?;

        Ok(default_nix)
    }

    fn prune_unneeded_crates(&mut self) {
        let mut queue: VecDeque<&PackageId> = self
            .root_package_id
            .iter()
            .chain(self.workspace_members.values())
            .collect();
        let mut reachable = HashSet::new();
        let indexed_crates: BTreeMap<_, _> =
            self.crates.iter().map(|c| (&c.package_id, c)).collect();
        while let Some(next_package_id) = queue.pop_back() {
            if !reachable.insert(next_package_id.clone()) {
                continue;
            }

            queue.extend(
                indexed_crates
                    .get(next_package_id)
                    .iter()
                    .flat_map(|c| {
                        c.dependencies
                            .iter()
                            .chain(c.build_dependencies.iter())
                            .chain(c.dev_dependencies.iter())
                    })
                    .map(|d| &d.package_id),
            );
        }
        self.crates.retain(|c| reachable.contains(&c.package_id));
    }

    fn new(
        info: &GenerateInfo,
        config: &GenerateConfig,
        metadata: IndexedMetadata,
    ) -> Result<BuildInfo, Error> {
        let crate2nix_json = crate::config::Config::read_from_or_default(
            &config
                .crate_hashes_json
                .parent()
                .expect("crate-hashes.json has parent dir")
                .join("crate2nix.json"),
        )?;

        Ok(BuildInfo {
            root_package_id: metadata.root.clone(),
            workspace_members: metadata
                .workspace_members
                .iter()
                .flat_map(|pkg_id| {
                    metadata
                        .pkgs_by_id
                        .get(pkg_id)
                        .map(|pkg| (pkg.name.clone(), pkg_id.clone()))
                })
                .collect(),
            crates: metadata
                .pkgs_by_id
                .values()
                .map(|package| {
                    CrateDerivation::resolve(config, &crate2nix_json, &metadata, package)
                })
                .collect::<Result<_, Error>>()?,
            indexed_metadata: metadata,
            info: info.clone(),
            config: config.clone(),
        })
    }
}

/// Call `cargo metadata` and return result.
fn cargo_metadata(config: &GenerateConfig, cargo_toml: &Path) -> Result<Metadata, Error> {
    let mut cmd = cargo_metadata::MetadataCommand::new();
    let mut other_options = config.other_metadata_options.clone();
    other_options.push("--locked".into());
    cmd.manifest_path(&cargo_toml)
        .other_options(&*other_options);
    cmd.exec().map_err(|e| {
        format_err!(
            "while retrieving metadata about {}: {}",
            &cargo_toml.to_string_lossy(),
            e
        )
    })
}

/// Prefetch hashes when necessary.
fn prefetch_and_fill_crates_sha256(
    config: &GenerateConfig,
    default_nix: &mut BuildInfo,
) -> Result<(), Error> {
    let mut from_lock_file: HashMap<PackageId, String> =
        extract_hashes_from_lockfile(&config, default_nix)?;
    for (_package_id, hash) in from_lock_file.iter_mut() {
        let bytes =
            hex::decode(&hash).map_err(|e| format_err!("while decoding '{}': {}", hash, e))?;
        *hash = nix_base32::to_nix_base32(&bytes);
    }

    let prefetched = prefetch::prefetch(
        config,
        &from_lock_file,
        &default_nix.crates,
        &default_nix.indexed_metadata.id_shortener,
    )
    .map_err(|e| format_err!("while prefetching crates for calculating sha256: {}", e))?;

    for package in default_nix.crates.iter_mut() {
        if package.source.sha256().is_none() {
            if let Some(hash) = prefetched
                .get(
                    default_nix
                        .indexed_metadata
                        .id_shortener
                        .lengthen_ref(&package.package_id),
                )
                .or_else(|| from_lock_file.get(&package.package_id))
            {
                package.source = package.source.with_sha256(hash.clone());
            }
        }
    }

    Ok(())
}

fn extract_hashes_from_lockfile(
    config: &GenerateConfig,
    default_nix: &mut BuildInfo,
) -> Result<HashMap<PackageId, String>, Error> {
    if !config.use_cargo_lock_checksums {
        return Ok(HashMap::new());
    }

    let mut hashes: HashMap<PackageId, String> = HashMap::new();

    for cargo_toml in &config.cargo_toml {
        let lock_file_path = cargo_toml.parent().unwrap().join("Cargo.lock");
        let lock_file = crate::lock::EncodableResolve::load_lock_file(&lock_file_path)?;
        lock_file
            .get_hashes_by_package_id(&mut hashes)
            .context(format!(
                "while parsing checksums from Lockfile {}",
                &lock_file_path.to_string_lossy()
            ))?;
    }

    let hashes_with_shortened_ids: HashMap<PackageId, String> = hashes
        .into_iter()
        .map(|(package_id, hash)| {
            (
                default_nix
                    .indexed_metadata
                    .id_shortener
                    .shorten_owned(package_id),
                hash,
            )
        })
        .collect();

    let mut missing_hashes = Vec::new();
    for package in default_nix.crates.iter_mut().filter(|c| match &c.source {
        ResolvedSource::CratesIo(CratesIoSource { sha256, .. }) if sha256.is_none() => {
            !hashes_with_shortened_ids.contains_key(&c.package_id)
        }
        _ => false,
    }) {
        missing_hashes.push(format!("{} {}", package.crate_name, package.version));
    }
    if !missing_hashes.is_empty() {
        eprintln!(
            "Did not find all crates.io hashes in Cargo.lock. Hashes for e.g. {} are missing.\n\
             This is probably a bug.",
            missing_hashes.iter().take(10).join(", ")
        );
    }
    Ok(hashes_with_shortened_ids)
}

/// Some info about the crate2nix invocation.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GenerateInfo {
    /// The version of this `crate2nix` instance.
    pub crate2nix_version: String,
    /// The arguments that were passed to `crate2nix`.
    pub crate2nix_arguments: Vec<String>,
}

impl Default for GenerateInfo {
    fn default() -> GenerateInfo {
        GenerateInfo {
            crate2nix_version: env!("CARGO_PKG_VERSION").to_string(),
            crate2nix_arguments: env::args().skip(1).collect(),
        }
    }
}

/// Configuration for the default.nix generation.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GenerateConfig {
    /// The path to `Cargo.toml`.
    pub cargo_toml: Vec<PathBuf>,
    /// Whether to inspect `Cargo.lock` for checksums so that we do not need to prefetch them.
    pub use_cargo_lock_checksums: bool,
    /// The path of the generated `Cargo.nix` file.
    pub output: PathBuf,
    /// The path of the `crate-hashes.json` file which is used to look up hashes and/or store
    /// prefetched hashes at.
    pub crate_hashes_json: PathBuf,
    /// The nix expression for the nixpkgs path to use.
    pub nixpkgs_path: String,
    /// Additional arguments to pass to `cargo metadata`.
    pub other_metadata_options: Vec<String>,
    /// Whether to read a `crate-hashes.json` file.
    pub read_crate_hashes: bool,
}
