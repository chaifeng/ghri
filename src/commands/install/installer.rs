use std::path::Path;

use crate::{
    package::Meta,
    source::{RepoId, SourceRelease},
};

use super::download::DownloadPlan;

/// Show installation plan to user (standalone function for use from mod.rs)
#[allow(clippy::too_many_arguments)]
pub fn show_install_plan(
    repo: &RepoId,
    release: &SourceRelease,
    target_dir: &Path,
    meta_path: &Path,
    plan: &DownloadPlan,
    needs_save: bool,
    meta: &Meta,
) {
    println!();
    println!("=== Installation Plan ===");
    println!();
    println!("Package:  {}", repo);
    println!("Version:  {}", release.tag);
    println!();

    // Show files to download
    println!("Files to download:");
    match plan {
        DownloadPlan::Tarball { url } => {
            println!("  - {} (source tarball)", url);
        }
        DownloadPlan::Assets { assets } => {
            for asset in assets {
                println!("  - {} ({} bytes)", asset.name, asset.size);
            }
        }
    }
    println!();

    // Show files/directories to create
    println!("Files/directories to create:");
    println!("  [DIR]  {}", target_dir.display());
    if needs_save {
        println!("  [FILE] {}", meta_path.display());
    } else {
        println!("  [MOD]  {} (update)", meta_path.display());
    }
    if let Some(parent) = target_dir.parent() {
        println!("  [LINK] {}/current -> {}", parent.display(), release.tag);
    }

    // Note: External link validation requires runtime, handled separately if needed
    if !meta.links.is_empty() {
        println!();
        println!("External links configured: {} link(s)", meta.links.len());
        for link in &meta.links {
            let source = link
                .path
                .as_ref()
                .map(|p| format!(":{}", p))
                .unwrap_or_default();
            println!(
                "  [LINK] {} -> {}{}/{}",
                link.dest.display(),
                repo,
                source,
                release.tag
            );
        }
    }

    // Show versioned links (these won't be updated)
    if !meta.versioned_links.is_empty() {
        println!();
        println!("Versioned links (unchanged):");
        for link in &meta.versioned_links {
            println!(
                "  [LINK] {} -> {}@{}",
                link.dest.display(),
                repo,
                link.version
            );
        }
    }

    println!();
}
