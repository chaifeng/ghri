use anyhow::Result;
use log::debug;
use std::path::PathBuf;

use crate::{
    package::{Meta, find_all_packages},
    runtime::Runtime,
};

use super::paths::default_install_root;

/// List all installed packages
#[tracing::instrument(skip(runtime, install_root))]
pub fn list<R: Runtime>(runtime: R, install_root: Option<PathBuf>) -> Result<()> {
    let root = match install_root {
        Some(path) => path,
        None => default_install_root(&runtime)?,
    };

    debug!("Listing packages from {:?}", root);

    let meta_files = find_all_packages(&runtime, &root)?;
    if meta_files.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    debug!("Found {} package(s)", meta_files.len());

    for meta_path in meta_files {
        match Meta::load(&runtime, &meta_path) {
            Ok(meta) => {
                let version = if meta.current_version.is_empty() {
                    "(unknown)".to_string()
                } else {
                    meta.current_version.clone()
                };
                println!("{} {}", meta.name, version);
            }
            Err(e) => {
                debug!("Failed to load meta from {:?}: {}", meta_path, e);
            }
        }
    }

    Ok(())
}
