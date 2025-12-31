use anyhow::{Context, Result};
use async_trait::async_trait;
use log::{debug, info};
#[cfg(test)]
use mockall::automock;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{
    archive::ArchiveExtractor,
    cleanup::CleanupContext,
    domain::model::{Release, ReleaseAsset},
    download::Downloader,
    provider::RepoId,
    runtime::Runtime,
};

/// Trait for installing a release to a target directory.
///
/// This abstracts the download and extraction logic, making it easy to mock
/// in tests without needing to mock low-level runtime operations.
#[cfg_attr(test, automock)]
#[async_trait]
pub trait ReleaseInstaller: Send + Sync {
    /// Install a release to the target directory.
    ///
    /// This will:
    /// 1. Check if target directory already exists (skip if so)
    /// 2. Filter assets based on provided patterns
    /// 3. Download either assets or source tarball
    /// 4. Extract archives as needed
    /// 5. Handle cleanup on failure or Ctrl-C
    async fn install(
        &self,
        repo: &RepoId,
        release: &Release,
        target_dir: &Path,
        filters: &[String],
        original_args: &[String],
    ) -> Result<()>;
}

/// Default implementation of ReleaseInstaller that downloads and extracts releases.
pub struct DefaultReleaseInstaller<R, D, E>
where
    R: Runtime + 'static,
    D: Downloader,
    E: ArchiveExtractor,
{
    runtime: Arc<R>,
    downloader: Arc<D>,
    extractor: Arc<E>,
    cleanup_ctx: Arc<Mutex<CleanupContext>>,
}

impl<R, D, E> DefaultReleaseInstaller<R, D, E>
where
    R: Runtime + 'static,
    D: Downloader,
    E: ArchiveExtractor,
{
    pub fn new(
        runtime: Arc<R>,
        downloader: Arc<D>,
        extractor: Arc<E>,
        cleanup_ctx: Arc<Mutex<CleanupContext>>,
    ) -> Self {
        Self {
            runtime,
            downloader,
            extractor,
            cleanup_ctx,
        }
    }
}

#[async_trait]
impl<R, D, E> ReleaseInstaller for DefaultReleaseInstaller<R, D, E>
where
    R: Runtime + 'static,
    D: Downloader + Send + Sync,
    E: ArchiveExtractor + Send + Sync,
{
    async fn install(
        &self,
        repo: &RepoId,
        release: &Release,
        target_dir: &Path,
        filters: &[String],
        original_args: &[String],
    ) -> Result<()> {
        ensure_installed_impl(
            self.runtime.as_ref(),
            target_dir,
            repo,
            release,
            self.downloader.as_ref(),
            self.extractor.as_ref(),
            Arc::clone(&self.cleanup_ctx),
            filters,
            original_args,
        )
        .await
    }
}

/// Describes what will be downloaded during installation
#[derive(Debug, Clone)]
pub enum DownloadPlan {
    /// Download source tarball (when no assets available)
    Tarball { url: String },
    /// Download release assets
    Assets { assets: Vec<ReleaseAsset> },
}

/// Get the download plan without actually downloading
pub fn get_download_plan(release: &Release, filters: &[String]) -> Result<DownloadPlan> {
    // Filter assets if filters are provided
    let filtered_assets = if filters.is_empty() {
        release.assets.clone()
    } else {
        filter_assets(&release.assets, filters)
    };

    // Error if assets exist but none matched the filters
    if !release.assets.is_empty() && !filters.is_empty() && filtered_assets.is_empty() {
        let mut asset_names: Vec<&str> = release.assets.iter().map(|a| a.name.as_str()).collect();
        asset_names.sort();
        let assets_list = asset_names.join("\n  ");

        anyhow::bail!(
            "No assets matched the filter patterns {:?}.\nAvailable assets:\n  {}",
            filters,
            assets_list
        );
    }

    if filtered_assets.is_empty() {
        Ok(DownloadPlan::Tarball {
            url: release.tarball_url.clone(),
        })
    } else {
        Ok(DownloadPlan::Assets {
            assets: filtered_assets,
        })
    }
}

#[tracing::instrument(skip(
    runtime,
    target_dir,
    repo,
    release,
    downloader,
    extractor,
    cleanup_ctx,
    filters,
    original_args
))]
#[allow(clippy::too_many_arguments)]
pub(super) async fn ensure_installed_impl<
    R: Runtime + 'static,
    E: ArchiveExtractor,
    D: Downloader,
>(
    runtime: &R,
    target_dir: &Path,
    repo: &RepoId,
    release: &Release,
    downloader: &D,
    extractor: &E,
    cleanup_ctx: Arc<Mutex<CleanupContext>>,
    filters: &[String],
    original_args: &[String],
) -> Result<()> {
    if runtime.exists(target_dir) {
        info!(
            "Directory {:?} already exists. Skipping download and extraction.",
            target_dir
        );
        return Ok(());
    }

    // Filter assets if filters are provided (check BEFORE creating directory)
    let filtered_assets = if filters.is_empty() {
        release.assets.clone()
    } else {
        filter_assets(&release.assets, filters)
    };

    // Error if assets exist but none matched the filters
    if !release.assets.is_empty() && !filters.is_empty() && filtered_assets.is_empty() {
        let mut asset_names: Vec<&str> = release.assets.iter().map(|a| a.name.as_str()).collect();
        asset_names.sort();
        let assets_list = asset_names.join("\n  ");

        // Check if any filter lacks wildcards and suggest using them
        let filters_without_wildcards: Vec<&str> = filters
            .iter()
            .filter(|f| !f.contains('*') && !f.contains('?'))
            .map(|s| s.as_str())
            .collect();

        let hint = if !filters_without_wildcards.is_empty() {
            // Generate suggested filters with wildcards
            let suggested_filters: Vec<String> = filters
                .iter()
                .map(|f| {
                    if f.contains('*') || f.contains('?') {
                        f.clone()
                    } else {
                        format!("*{}*", f)
                    }
                })
                .collect();

            // Build suggested command from original args, replacing filter values
            let suggested_command = build_suggested_command(original_args, &suggested_filters);

            format!(
                "\n\nHint: Your filter(s) {:?} don't contain wildcards.\n\
                Try using wildcards for partial matching:\n  \
                {}",
                filters_without_wildcards, suggested_command
            )
        } else {
            String::new()
        };

        anyhow::bail!(
            "No assets matched the filter patterns {:?}.\nAvailable assets:\n  {}{}",
            filters,
            assets_list,
            hint
        );
    }

    debug!("Creating target directory: {:?}", target_dir);
    runtime
        .create_dir_all(target_dir)
        .with_context(|| format!("Failed to create target directory at {:?}", target_dir))?;

    // Register target_dir for cleanup on Ctrl-C
    {
        let mut ctx = cleanup_ctx.lock().unwrap();
        ctx.add(target_dir.to_path_buf());
    }

    // Create a modified release with filtered assets
    let filtered_release = Release {
        assets: filtered_assets,
        ..release.clone()
    };

    // Choose download strategy based on filtered assets availability
    if filtered_release.assets.is_empty() {
        // No assets: download source tarball
        download_and_extract_tarball(
            runtime,
            target_dir,
            repo,
            &filtered_release,
            downloader,
            extractor,
            Arc::clone(&cleanup_ctx),
        )
        .await?;
    } else {
        // Has assets: download all asset files
        download_all_assets(
            runtime,
            target_dir,
            repo,
            &filtered_release,
            downloader,
            extractor,
            Arc::clone(&cleanup_ctx),
        )
        .await?;
    }

    // Installation succeeded, remove target_dir from cleanup list
    {
        let mut ctx = cleanup_ctx.lock().unwrap();
        ctx.remove(target_dir);
    }

    Ok(())
}

/// Filter assets by glob patterns. An asset matches if ANY pattern matches its name (OR logic).
/// If filters is empty, returns all assets.
fn filter_assets(assets: &[ReleaseAsset], filters: &[String]) -> Vec<ReleaseAsset> {
    if filters.is_empty() {
        return assets.to_vec();
    }
    assets
        .iter()
        .filter(|asset| {
            filters.iter().any(|pattern| {
                glob::Pattern::new(pattern)
                    .map(|p| p.matches(&asset.name))
                    .unwrap_or(false)
            })
        })
        .cloned()
        .collect()
}

/// Download source tarball (when no assets available)
/// Since it's a single file that is an archive, extract it
async fn download_and_extract_tarball<R: Runtime + 'static, E: ArchiveExtractor, D: Downloader>(
    runtime: &R,
    target_dir: &Path,
    repo: &RepoId,
    release: &Release,
    downloader: &D,
    extractor: &E,
    cleanup_ctx: Arc<Mutex<CleanupContext>>,
) -> Result<()> {
    let temp_dir = runtime.temp_dir();
    let temp_file_path = temp_dir.join(format!("{}-{}.tar.gz", repo.repo, release.tag));

    println!(" downloading {} {} (source)", &repo, release.tag);
    println!(
        " downloading {} {} -> {}",
        &repo, release.tag, release.tarball_url
    );
    if let Err(e) = downloader
        .download(runtime, &release.tarball_url, &temp_file_path)
        .await
    {
        debug!(
            "Download failed, cleaning up target directory: {:?}",
            target_dir
        );
        let _ = runtime.remove_dir_all(target_dir);
        return Err(e);
    }

    // Register temp file for cleanup (after download succeeds)
    {
        let mut ctx = cleanup_ctx.lock().unwrap();
        ctx.add(temp_file_path.clone());
    }

    // Single file downloaded and it's an archive, so extract it
    println!("  installing {} {}", &repo, release.tag);
    if let Err(e) = extractor.extract_with_cleanup(
        runtime,
        &temp_file_path,
        target_dir,
        Arc::clone(&cleanup_ctx),
    ) {
        debug!(
            "Extraction failed, cleaning up target directory: {:?}",
            target_dir
        );
        let _ = runtime.remove_dir_all(target_dir);
        let _ = runtime.remove_file(&temp_file_path);
        return Err(e);
    }

    // Remove temp file from cleanup list and delete it
    {
        let mut ctx = cleanup_ctx.lock().unwrap();
        ctx.remove(&temp_file_path);
    }
    runtime
        .remove_file(&temp_file_path)
        .with_context(|| format!("Failed to clean up temporary file: {:?}", temp_file_path))?;

    Ok(())
}

/// Build a suggested command by replacing filter values in the original command line args
fn build_suggested_command(original_args: &[String], suggested_filters: &[String]) -> String {
    let mut result = Vec::new();
    let mut i = 0;
    let mut filter_index = 0;

    while i < original_args.len() {
        let arg = &original_args[i];

        if arg == "--filter" || arg == "-f" {
            // Skip the original filter and its value, use suggested filter instead
            result.push(arg.clone());
            if filter_index < suggested_filters.len() {
                result.push(format!("\"{}\"", suggested_filters[filter_index]));
                filter_index += 1;
            }
            i += 2; // Skip both --filter and its value
        } else if arg.starts_with("--filter=") {
            // Handle --filter=value format
            if filter_index < suggested_filters.len() {
                result.push(format!("--filter=\"{}\"", suggested_filters[filter_index]));
                filter_index += 1;
            }
            i += 1;
        } else if arg.starts_with("-f=") {
            // Handle -f=value format
            if filter_index < suggested_filters.len() {
                result.push(format!("-f=\"{}\"", suggested_filters[filter_index]));
                filter_index += 1;
            }
            i += 1;
        } else {
            result.push(arg.clone());
            i += 1;
        }
    }

    result.join(" ")
}

/// Check if a filename represents an archive that can be extracted
fn is_archive(name: &str) -> bool {
    let name_lower = name.to_lowercase();
    name_lower.ends_with(".tar.gz")
        || name_lower.ends_with(".tgz")
        || name_lower.ends_with(".tar.xz")
        || name_lower.ends_with(".tar.bz2")
        || name_lower.ends_with(".zip")
}

/// Check if a file is a native binary executable for the current platform.
/// Uses goblin to parse the binary format and determine if it's executable.
/// Returns true only for native executables matching the current platform:
/// - Linux: ELF binaries only
/// - macOS: Mach-O binaries only (including universal/fat binaries)
///
/// Scripts and binaries for other platforms are not considered native executables.
#[cfg(unix)]
fn is_native_executable<R: Runtime>(runtime: &R, path: &Path) -> bool {
    use std::io::Read;

    let mut file = match runtime.open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    // Read enough bytes for goblin to detect the format
    let mut buffer = Vec::new();
    if file.read_to_end(&mut buffer).is_err() {
        return false;
    }

    match goblin::Object::parse(&buffer) {
        #[cfg(target_os = "linux")]
        Ok(goblin::Object::Elf(_)) => true,
        #[cfg(target_os = "macos")]
        Ok(goblin::Object::Mach(_)) => true,
        _ => false,
    }
}

/// Set executable permission on a file if it's a native binary.
/// This is a no-op on Windows.
#[cfg(unix)]
fn set_executable_if_binary<R: Runtime>(runtime: &R, path: &Path) -> Result<()> {
    if is_native_executable(runtime, path) {
        debug!("Setting executable permission on {:?}", path);
        runtime.set_permissions(path, 0o755)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_executable_if_binary<R: Runtime>(_runtime: &R, _path: &Path) -> Result<()> {
    // No-op on Windows
    Ok(())
}

/// Download all release assets (when assets are available)
/// If only one file is downloaded and it's an archive, extract it.
/// If multiple files are downloaded, keep them as-is without extraction.
async fn download_all_assets<R: Runtime + 'static, E: ArchiveExtractor, D: Downloader>(
    runtime: &R,
    target_dir: &Path,
    repo: &RepoId,
    release: &Release,
    downloader: &D,
    extractor: &E,
    cleanup_ctx: Arc<Mutex<CleanupContext>>,
) -> Result<()> {
    let temp_dir = runtime.temp_dir();
    let mut temp_files: Vec<PathBuf> = Vec::new();

    let assets_count: usize = release.assets.len();
    println!(
        " downloading {} {} ({} assets)",
        &repo, release.tag, assets_count
    );

    // Download all assets
    let mut asset_index = 0;
    for asset in &release.assets {
        asset_index += 1;
        let temp_file_path =
            temp_dir.join(format!("{}-{}-{}", repo.repo, release.tag, &asset.name));

        debug!(
            "Downloading asset: {}({}) -> {:?}",
            &asset.name, &asset.download_url, &temp_file_path
        );
        println!(
            " downloading {} {} ({}/{} assets) -> {}",
            &repo, release.tag, asset_index, assets_count, &asset.download_url
        );
        if let Err(e) = downloader
            .download(runtime, &asset.download_url, &temp_file_path)
            .await
        {
            debug!("Download failed for asset {}, cleaning up", asset.name);
            // Clean up already downloaded temp files
            for temp_file in &temp_files {
                let _ = runtime.remove_file(temp_file);
            }
            let _ = runtime.remove_dir_all(target_dir);
            return Err(e.context(format!("Failed to download asset: {}", asset.name)));
        }

        temp_files.push(temp_file_path.clone());

        // Register temp file for cleanup
        {
            let mut ctx = cleanup_ctx.lock().unwrap();
            ctx.add(temp_file_path);
        }
    }

    println!("  installing {} {}", &repo, release.tag);

    // Only extract if there's exactly one file and it's an archive
    // Otherwise, copy all files as-is to target directory
    let should_extract = release.assets.len() == 1 && is_archive(&release.assets[0].name);

    if should_extract {
        // Single archive file: extract it
        let temp_file_path = &temp_files[0];
        let asset = &release.assets[0];
        debug!(
            "Extracting single asset: {} -> {:?}",
            asset.name, target_dir
        );
        if let Err(e) = extractor.extract_with_cleanup(
            runtime,
            temp_file_path,
            target_dir,
            Arc::clone(&cleanup_ctx),
        ) {
            debug!("Extraction failed for asset {}, cleaning up", asset.name);
            let _ = runtime.remove_file(temp_file_path);
            let _ = runtime.remove_dir_all(target_dir);
            return Err(e.context(format!("Failed to extract asset: {}", asset.name)));
        }
    } else {
        // Multiple files or single non-archive file: copy all as-is
        for (i, asset) in release.assets.iter().enumerate() {
            let temp_file_path = &temp_files[i];
            let dest_path = target_dir.join(&asset.name);
            debug!("Copying asset: {} -> {:?}", asset.name, dest_path);
            if let Err(e) = runtime.copy(temp_file_path, &dest_path) {
                debug!("Copy failed for asset {}, cleaning up", asset.name);
                for temp_file in &temp_files {
                    let _ = runtime.remove_file(temp_file);
                }
                let _ = runtime.remove_dir_all(target_dir);
                return Err(e.context(format!("Failed to copy asset: {}", asset.name)));
            }

            // Set executable permission if the file is a native binary (Unix only)
            if let Err(e) = set_executable_if_binary(runtime, &dest_path) {
                debug!(
                    "Failed to set executable permission on {:?}: {}",
                    dest_path, e
                );
                // Non-fatal: continue even if permission setting fails
            }
        }
    }

    // Clean up all temp files
    for temp_file in &temp_files {
        {
            let mut ctx = cleanup_ctx.lock().unwrap();
            ctx.remove(temp_file);
        }
        runtime
            .remove_file(temp_file)
            .with_context(|| format!("Failed to clean up temporary file: {:?}", temp_file))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::MockArchiveExtractor;
    use crate::download::HttpDownloader;
    use crate::http::HttpClient;
    use crate::runtime::MockRuntime;
    use mockall::predicate::*;
    use reqwest::Client;
    use std::path::PathBuf;

    #[test]
    fn test_filter_assets_single_pattern() {
        // Test filtering assets with a single glob pattern
        // Pattern: "*aarch64*" should match assets containing "aarch64"

        let assets = vec![
            ReleaseAsset {
                name: "app-linux-x86_64.tar.gz".into(),
                size: 1000,
                download_url: "http://example.com/x86_64".into(),
            },
            ReleaseAsset {
                name: "app-linux-aarch64.tar.gz".into(),
                size: 1000,
                download_url: "http://example.com/aarch64".into(),
            },
            ReleaseAsset {
                name: "app-darwin-aarch64.tar.gz".into(),
                size: 1000,
                download_url: "http://example.com/darwin-aarch64".into(),
            },
        ];

        let filters = vec!["*aarch64*".to_string()];
        let filtered = filter_assets(&assets, &filters);

        // Should match: app-linux-aarch64.tar.gz, app-darwin-aarch64.tar.gz
        assert_eq!(filtered.len(), 2);
        assert!(
            filtered
                .iter()
                .any(|a| a.name == "app-linux-aarch64.tar.gz")
        );
        assert!(
            filtered
                .iter()
                .any(|a| a.name == "app-darwin-aarch64.tar.gz")
        );
    }

    #[test]
    fn test_filter_assets_multiple_patterns_or_logic() {
        // Test filtering assets with multiple patterns (OR logic)
        // Pattern: "*aarch64*" OR "*x86_64*" should match both architectures

        let assets = vec![
            ReleaseAsset {
                name: "app-linux-x86_64.tar.gz".into(),
                size: 1000,
                download_url: "http://example.com/x86_64".into(),
            },
            ReleaseAsset {
                name: "app-linux-aarch64.tar.gz".into(),
                size: 1000,
                download_url: "http://example.com/aarch64".into(),
            },
            ReleaseAsset {
                name: "app-darwin-aarch64.tar.gz".into(),
                size: 1000,
                download_url: "http://example.com/darwin-aarch64".into(),
            },
            ReleaseAsset {
                name: "app-darwin-x86_64.tar.gz".into(),
                size: 1000,
                download_url: "http://example.com/darwin-x86_64".into(),
            },
            ReleaseAsset {
                name: "checksums.txt".into(),
                size: 100,
                download_url: "http://example.com/checksums".into(),
            },
        ];

        let filters = vec!["*aarch64*".to_string(), "*x86_64*".to_string()];
        let filtered = filter_assets(&assets, &filters);

        // Should match all assets containing aarch64 OR x86_64 (4 total)
        assert_eq!(filtered.len(), 4);
        assert!(filtered.iter().any(|a| a.name == "app-linux-x86_64.tar.gz"));
        assert!(
            filtered
                .iter()
                .any(|a| a.name == "app-linux-aarch64.tar.gz")
        );
        assert!(
            filtered
                .iter()
                .any(|a| a.name == "app-darwin-aarch64.tar.gz")
        );
        assert!(
            filtered
                .iter()
                .any(|a| a.name == "app-darwin-x86_64.tar.gz")
        );
        // checksums.txt should NOT be included
        assert!(!filtered.iter().any(|a| a.name == "checksums.txt"));
    }

    #[test]
    fn test_filter_assets_no_match() {
        // Test filtering when no assets match the pattern

        let assets = vec![ReleaseAsset {
            name: "app-linux-x86_64.tar.gz".into(),
            size: 1000,
            download_url: "http://example.com/x86_64".into(),
        }];

        let filters = vec!["*windows*".to_string()];
        let filtered = filter_assets(&assets, &filters);

        // Should return empty vec (no match)
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_filter_assets_empty_filters() {
        // Test that empty filters returns all assets

        let assets = vec![
            ReleaseAsset {
                name: "app-linux-x86_64.tar.gz".into(),
                size: 1000,
                download_url: "http://example.com/x86_64".into(),
            },
            ReleaseAsset {
                name: "app-darwin-aarch64.tar.gz".into(),
                size: 1000,
                download_url: "http://example.com/darwin-aarch64".into(),
            },
        ];

        let filters: Vec<String> = vec![];
        let filtered = filter_assets(&assets, &filters);

        // Should return all assets when no filters
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_build_suggested_command_with_filter_option() {
        // Test that build_suggested_command correctly replaces filter values
        let original_args = vec![
            "ghri".to_string(),
            "install".to_string(),
            "owner/repo".to_string(),
            "--filter".to_string(),
            "linux".to_string(),
        ];
        let suggested_filters = vec!["*linux*".to_string()];

        let result = build_suggested_command(&original_args, &suggested_filters);

        assert_eq!(result, "ghri install owner/repo --filter \"*linux*\"");
    }

    #[test]
    fn test_build_suggested_command_with_short_filter_option() {
        // Test with -f short option
        let original_args = vec![
            "ghri".to_string(),
            "install".to_string(),
            "owner/repo".to_string(),
            "-f".to_string(),
            "darwin".to_string(),
        ];
        let suggested_filters = vec!["*darwin*".to_string()];

        let result = build_suggested_command(&original_args, &suggested_filters);

        assert_eq!(result, "ghri install owner/repo -f \"*darwin*\"");
    }

    #[test]
    fn test_build_suggested_command_with_equals_format() {
        // Test with --filter=value format
        let original_args = vec![
            "ghri".to_string(),
            "install".to_string(),
            "owner/repo".to_string(),
            "--filter=linux".to_string(),
        ];
        let suggested_filters = vec!["*linux*".to_string()];

        let result = build_suggested_command(&original_args, &suggested_filters);

        assert_eq!(result, "ghri install owner/repo --filter=\"*linux*\"");
    }

    #[test]
    fn test_build_suggested_command_with_multiple_filters() {
        // Test with multiple filter options
        let original_args = vec![
            "ghri".to_string(),
            "install".to_string(),
            "owner/repo".to_string(),
            "--filter".to_string(),
            "linux".to_string(),
            "-f".to_string(),
            "x86_64".to_string(),
        ];
        let suggested_filters = vec!["*linux*".to_string(), "*x86_64*".to_string()];

        let result = build_suggested_command(&original_args, &suggested_filters);

        assert_eq!(
            result,
            "ghri install owner/repo --filter \"*linux*\" -f \"*x86_64*\""
        );
    }

    #[test]
    fn test_build_suggested_command_empty_args() {
        // Test with empty original args
        let original_args: Vec<String> = vec![];
        let suggested_filters = vec!["*linux*".to_string()];

        let result = build_suggested_command(&original_args, &suggested_filters);

        assert_eq!(result, "");
    }

    #[tokio::test]
    async fn test_ensure_installed_creates_dir_and_extracts() {
        // Test successful installation: creates directory, downloads, and extracts

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup ---
        let target = PathBuf::from("/target"); // Installation target directory
        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag: "v1".into(),
            tarball_url: "http://mock/tar".into(),
            ..Default::default()
        };

        // --- 1. Check if Already Installed ---

        // Directory exists: /target -> false (not yet installed)
        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| false);

        // --- 2. Create Target Directory ---

        // Create directory: /target
        runtime
            .expect_create_dir_all()
            .with(eq(target.clone()))
            .returning(|_| Ok(()));

        // --- 3. Download Archive ---

        // Create temp file for download
        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));

        // Remove temp file after extraction
        runtime.expect_remove_file().returning(|_| Ok(()));

        // --- 4. Extract Archive ---

        let mut extractor = MockArchiveExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        // --- Setup Mock HTTP Server ---

        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/tar").with_status(200).create();
        let release_with_url = Release {
            tarball_url: format!("{}/tar", server.url()),
            ..release
        };

        // --- Execute ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let downloader = HttpDownloader::new(HttpClient::new(Client::new()));
        ensure_installed_impl(
            &runtime,
            &target,
            &repo,
            &release_with_url,
            &downloader,
            &extractor,
            cleanup_ctx,
            &[],
            &[],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_ensure_installed_cleanup_fail() {
        // Test that cleanup failure (removing temp file) returns an error

        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup ---
        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag: "v1".into(),
            tarball_url: format!("{}/download", url),
            ..Default::default()
        };
        let target_dir = PathBuf::from("/tmp/target");

        // Mock server returns success for download
        let _m = server.mock("GET", "/download").with_status(200).create();

        // --- 1. Check if Already Installed ---

        // Directory exists: /tmp/target -> false
        runtime
            .expect_exists()
            .with(eq(target_dir.clone()))
            .returning(|_| false);

        // --- 2. Create Target Directory ---

        // Create directory: /tmp/target
        runtime
            .expect_create_dir_all()
            .with(eq(target_dir.clone()))
            .returning(|_| Ok(()));

        // --- 3. Download Archive ---

        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));

        // --- 4. Remove Temp File FAILS ---

        // Remove temp file: -> ERROR (cleanup fails!)
        runtime
            .expect_remove_file()
            .returning(|_| Err(anyhow::anyhow!("fail")));

        // --- 5. Extract Archive (succeeds) ---

        let mut extractor = MockArchiveExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        // --- Execute & Verify ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let downloader = HttpDownloader::new(HttpClient::new(Client::new()));
        let result = ensure_installed_impl(
            &runtime,
            &target_dir,
            &repo,
            &release,
            &downloader,
            &extractor,
            cleanup_ctx,
            &[],
            &[],
        )
        .await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to clean up temporary file")
        );
    }

    #[tokio::test]
    async fn test_ensure_installed_download_fail_cleans_up_target_dir() {
        // Test that download failure cleans up the created target directory

        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup ---
        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag: "v1".into(),
            tarball_url: format!("{}/download", url),
            ..Default::default()
        };
        let target_dir = PathBuf::from("/tmp/target");

        // Mock server returns 404 (download will fail)
        let _m = server.mock("GET", "/download").with_status(404).create();

        // --- 1. Check if Already Installed ---

        // Directory exists: /tmp/target -> false
        runtime
            .expect_exists()
            .with(eq(target_dir.clone()))
            .returning(|_| false);

        // --- 2. Create Target Directory ---

        // Create directory: /tmp/target
        runtime
            .expect_create_dir_all()
            .with(eq(target_dir.clone()))
            .returning(|_| Ok(()));

        // --- 3. Download Fails -> Cleanup Target Directory ---

        // Remove directory: /tmp/target (cleanup on failure)
        runtime
            .expect_remove_dir_all()
            .with(eq(target_dir.clone()))
            .times(1)
            .returning(|_| Ok(()));

        let extractor = MockArchiveExtractor::new();

        // --- Execute & Verify ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let downloader = HttpDownloader::new(HttpClient::new(Client::new()));
        let result = ensure_installed_impl(
            &runtime,
            &target_dir,
            &repo,
            &release,
            &downloader,
            &extractor,
            cleanup_ctx,
            &[],
            &[],
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ensure_installed_extract_fail_cleans_up_target_dir() {
        // Test that extraction failure cleans up both target directory and temp file

        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup ---
        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag: "v1".into(),
            tarball_url: format!("{}/download", url),
            ..Default::default()
        };
        let target_dir = PathBuf::from("/tmp/target");

        // Mock server returns success
        let _m = server
            .mock("GET", "/download")
            .with_status(200)
            .with_body("data")
            .create();

        // --- 1. Check if Already Installed ---

        // Directory exists: /tmp/target -> false
        runtime
            .expect_exists()
            .with(eq(target_dir.clone()))
            .returning(|_| false);

        // --- 2. Create Target Directory ---

        // Create directory: /tmp/target
        runtime
            .expect_create_dir_all()
            .with(eq(target_dir.clone()))
            .returning(|_| Ok(()));

        // --- 3. Download Archive ---

        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));

        // --- 4. Extract Archive FAILS ---

        let mut extractor = MockArchiveExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Err(anyhow::anyhow!("extraction failed")));

        // --- 5. Cleanup on Failure ---

        // Remove directory: /tmp/target
        runtime
            .expect_remove_dir_all()
            .with(eq(target_dir.clone()))
            .times(1)
            .returning(|_| Ok(()));

        // Remove temp file
        runtime.expect_remove_file().times(1).returning(|_| Ok(()));

        // --- Execute & Verify ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let downloader = HttpDownloader::new(HttpClient::new(Client::new()));
        let result = ensure_installed_impl(
            &runtime,
            &target_dir,
            &repo,
            &release,
            &downloader,
            &extractor,
            cleanup_ctx,
            &[],
            &[],
        )
        .await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("extraction failed")
        );
    }

    #[tokio::test]
    async fn test_ensure_installed_already_exists() {
        // Test that installation is skipped when target directory already exists

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup ---
        let target = PathBuf::from("/target");

        // --- 1. Check if Already Installed ---

        // Directory exists: /target -> true (already installed!)
        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| true);

        // (No other operations should be performed)

        // --- Execute ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let downloader = HttpDownloader::new(HttpClient::new(Client::new()));
        let result = ensure_installed_impl(
            &runtime,
            &target,
            &RepoId {
                owner: "o".into(),
                repo: "r".into(),
            },
            &Release::default(),
            &downloader,
            &MockArchiveExtractor::new(),
            cleanup_ctx,
            &[],
            &[],
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_ensure_installed_with_multiple_assets_copies_all_without_extraction() {
        // Test installation with multiple assets: all files are copied as-is without extraction
        // Rule: When multiple files are downloaded, keep them as-is (no extraction)

        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup ---
        let target = PathBuf::from("/target");
        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };

        // Release with 2 assets (archive + text file)
        // Even though one is an archive, since there are multiple files, NONE are extracted
        let release = Release {
            tag: "v1".into(),
            tarball_url: format!("{}/tarball", url), // Should NOT be downloaded
            assets: vec![
                ReleaseAsset {
                    name: "app-linux-x86_64.tar.gz".into(),
                    size: 1000,
                    download_url: format!("{}/asset1.tar.gz", url),
                },
                ReleaseAsset {
                    name: "checksums.txt".into(),
                    size: 100,
                    download_url: format!("{}/checksums.txt", url),
                },
            ],
            ..Default::default()
        };

        // Mock server endpoints for assets (NOT tarball)
        let _m1 = server
            .mock("GET", "/asset1.tar.gz")
            .with_status(200)
            .with_body("archive data")
            .create();
        let _m2 = server
            .mock("GET", "/checksums.txt")
            .with_status(200)
            .with_body("sha256sum")
            .create();

        // --- 1. Check if Already Installed ---

        // Directory exists: /target -> false
        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| false);

        // --- 2. Create Target Directory ---

        // Create directory: /target
        runtime
            .expect_create_dir_all()
            .with(eq(target.clone()))
            .returning(|_| Ok(()));

        // --- 3. Download Assets ---

        // Create temp files for both assets
        runtime
            .expect_create_file()
            .times(2)
            .returning(|_| Ok(Box::new(std::io::sink())));

        // --- 4. Copy All Assets (NO extraction for multiple files) ---

        // Extractor should NOT be called - multiple files means no extraction
        let extractor = MockArchiveExtractor::new();

        // Both files are copied directly to target directory
        // Copy: /tmp/r-v1-app-linux-x86_64.tar.gz -> /target/app-linux-x86_64.tar.gz
        // Copy: /tmp/r-v1-checksums.txt -> /target/checksums.txt
        runtime.expect_copy().times(2).returning(|_, _| Ok(100));

        // --- 4.5. Check if Files are Native Binaries (Unix only) ---

        // Open files to check magic bytes (for set_executable_if_binary)
        // These are not native binaries, so no permission change needed
        #[cfg(unix)]
        runtime
            .expect_open()
            .times(2)
            .returning(|_| Ok(Box::new(std::io::Cursor::new(b"not a binary".to_vec()))));

        // --- 5. Cleanup Temp Files ---

        // Remove both temp files after processing
        runtime.expect_remove_file().times(2).returning(|_| Ok(()));

        // --- Execute ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let downloader = HttpDownloader::new(HttpClient::new(Client::new()));
        let result = ensure_installed_impl(
            &runtime,
            &target,
            &repo,
            &release,
            &downloader,
            &extractor,
            cleanup_ctx,
            &[],
            &[],
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_ensure_installed_with_single_archive_asset_extracts() {
        // Test installation with single archive asset: extract it
        // Rule: When only one file is downloaded and it's an archive, extract it

        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup ---
        let target = PathBuf::from("/target");
        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };

        // Release with single archive asset
        let release = Release {
            tag: "v1".into(),
            tarball_url: format!("{}/tarball", url), // Should NOT be downloaded
            assets: vec![ReleaseAsset {
                name: "app-linux-x86_64.tar.gz".into(),
                size: 1000,
                download_url: format!("{}/asset1.tar.gz", url),
            }],
            ..Default::default()
        };

        // Mock server endpoint for the single asset
        let _m1 = server
            .mock("GET", "/asset1.tar.gz")
            .with_status(200)
            .with_body("archive data")
            .create();

        // --- 1. Check if Already Installed ---

        // Directory exists: /target -> false
        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| false);

        // --- 2. Create Target Directory ---

        // Create directory: /target
        runtime
            .expect_create_dir_all()
            .with(eq(target.clone()))
            .returning(|_| Ok(()));

        // --- 3. Download Single Asset ---

        // Create temp file for the asset
        runtime
            .expect_create_file()
            .times(1)
            .returning(|_| Ok(Box::new(std::io::sink())));

        // --- 4. Extract Single Archive Asset ---

        let mut extractor = MockArchiveExtractor::new();
        // Single archive should be extracted
        extractor
            .expect_extract_with_cleanup()
            .times(1)
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        // --- 5. Cleanup Temp File ---

        // Remove temp file after extraction
        runtime.expect_remove_file().times(1).returning(|_| Ok(()));

        // --- Execute ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let downloader = HttpDownloader::new(HttpClient::new(Client::new()));
        let result = ensure_installed_impl(
            &runtime,
            &target,
            &repo,
            &release,
            &downloader,
            &extractor,
            cleanup_ctx,
            &[],
            &[],
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_ensure_installed_with_single_non_archive_asset_copies() {
        // Test installation with single non-archive asset: copy it directly
        // Rule: When only one file is downloaded but it's NOT an archive, copy it

        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup ---
        let target = PathBuf::from("/target");
        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };

        // Release with single non-archive asset (a binary)
        let release = Release {
            tag: "v1".into(),
            tarball_url: format!("{}/tarball", url), // Should NOT be downloaded
            assets: vec![ReleaseAsset {
                name: "app-linux-x86_64".into(), // No archive extension
                size: 1000,
                download_url: format!("{}/binary", url),
            }],
            ..Default::default()
        };

        // Mock server endpoint for the single binary
        let _m1 = server
            .mock("GET", "/binary")
            .with_status(200)
            .with_body("binary data")
            .create();

        // --- 1. Check if Already Installed ---

        // Directory exists: /target -> false
        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| false);

        // --- 2. Create Target Directory ---

        // Create directory: /target
        runtime
            .expect_create_dir_all()
            .with(eq(target.clone()))
            .returning(|_| Ok(()));

        // --- 3. Download Single Asset ---

        // Create temp file for the asset
        runtime
            .expect_create_file()
            .times(1)
            .returning(|_| Ok(Box::new(std::io::sink())));

        // --- 4. Copy Single Non-Archive Asset (NO extraction) ---

        // Extractor should NOT be called - it's not an archive
        let extractor = MockArchiveExtractor::new();

        // Copy: /tmp/r-v1-app-linux-x86_64 -> /target/app-linux-x86_64
        runtime.expect_copy().times(1).returning(|_, _| Ok(100));

        // --- 4.5. Check if File is Native Binary (Unix only) ---

        // Open file to check magic bytes (for set_executable_if_binary)
        // This is not a native binary (no ELF/Mach-O magic), so no permission change
        #[cfg(unix)]
        runtime
            .expect_open()
            .times(1)
            .returning(|_| Ok(Box::new(std::io::Cursor::new(b"not a binary".to_vec()))));

        // --- 5. Cleanup Temp File ---

        // Remove temp file after copy
        runtime.expect_remove_file().times(1).returning(|_| Ok(()));

        // --- Execute ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let downloader = HttpDownloader::new(HttpClient::new(Client::new()));
        let result = ensure_installed_impl(
            &runtime,
            &target,
            &repo,
            &release,
            &downloader,
            &extractor,
            cleanup_ctx,
            &[],
            &[],
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_ensure_installed_with_empty_assets_uses_tarball() {
        // Test that empty assets list falls back to downloading source tarball

        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup ---
        let target = PathBuf::from("/target");
        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };

        // Release with NO assets (empty vec)
        let release = Release {
            tag: "v1".into(),
            tarball_url: format!("{}/tarball.tar.gz", url), // Should be downloaded
            assets: vec![],                                 // Empty!
            ..Default::default()
        };

        // Mock server endpoint for tarball
        let mock_tarball = server
            .mock("GET", "/tarball.tar.gz")
            .with_status(200)
            .with_body("source")
            .create();

        // --- 1. Check if Already Installed ---

        // Directory exists: /target -> false
        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| false);

        // --- 2. Create Target Directory ---

        runtime
            .expect_create_dir_all()
            .with(eq(target.clone()))
            .returning(|_| Ok(()));

        // --- 3. Download Tarball (not assets) ---

        runtime
            .expect_create_file()
            .times(1)
            .returning(|_| Ok(Box::new(std::io::sink())));

        // --- 4. Extract Tarball ---

        let mut extractor = MockArchiveExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .times(1)
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        // --- 5. Cleanup Temp File ---

        runtime.expect_remove_file().times(1).returning(|_| Ok(()));

        // --- Execute ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let downloader = HttpDownloader::new(HttpClient::new(Client::new()));
        let result = ensure_installed_impl(
            &runtime,
            &target,
            &repo,
            &release,
            &downloader,
            &extractor,
            cleanup_ctx,
            &[],
            &[],
        )
        .await;

        // Verify tarball was downloaded
        mock_tarball.assert();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_ensure_installed_asset_download_failure_cleans_up() {
        // Test that asset download failure cleans up partial downloads and target directory

        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup ---
        let target = PathBuf::from("/target");
        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };

        // Release with 2 assets
        let release = Release {
            tag: "v1".into(),
            tarball_url: format!("{}/tarball", url),
            assets: vec![
                ReleaseAsset {
                    name: "asset1.tar.gz".into(),
                    size: 1000,
                    download_url: format!("{}/asset1.tar.gz", url),
                },
                ReleaseAsset {
                    name: "asset2.tar.gz".into(),
                    size: 2000,
                    download_url: format!("{}/asset2.tar.gz", url), // This will fail
                },
            ],
            ..Default::default()
        };

        // First asset downloads successfully, second fails with 404
        let _m1 = server
            .mock("GET", "/asset1.tar.gz")
            .with_status(200)
            .with_body("data")
            .create();
        let _m2 = server
            .mock("GET", "/asset2.tar.gz")
            .with_status(404)
            .create();

        // --- 1. Check if Already Installed ---

        // Directory exists: /target -> false
        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| false);

        // --- 2. Create Target Directory ---

        runtime
            .expect_create_dir_all()
            .with(eq(target.clone()))
            .returning(|_| Ok(()));

        // --- 3. Download First Asset (succeeds) ---

        runtime
            .expect_create_file()
            .times(1)
            .returning(|_| Ok(Box::new(std::io::sink())));

        // --- 4. Second Asset Download Fails -> Cleanup ---

        // Remove first temp file (cleanup on failure)
        runtime.expect_remove_file().times(1).returning(|_| Ok(()));

        // Remove target directory (cleanup on failure)
        runtime
            .expect_remove_dir_all()
            .with(eq(target.clone()))
            .times(1)
            .returning(|_| Ok(()));

        let extractor = MockArchiveExtractor::new();

        // --- Execute & Verify ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let downloader = HttpDownloader::new(HttpClient::new(Client::new()));
        let result = ensure_installed_impl(
            &runtime,
            &target,
            &repo,
            &release,
            &downloader,
            &extractor,
            cleanup_ctx,
            &[],
            &[],
        )
        .await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to download asset")
        );
    }

    #[tokio::test]
    async fn test_ensure_installed_error_when_filter_matches_nothing() {
        // Test that when assets exist but filter patterns match nothing, an error is returned
        // instead of falling back to downloading source tarball

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup ---
        let target = PathBuf::from("/target"); // Installation target directory
        let repo = RepoId {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        let release = Release {
            tag: "v1.0.0".into(),
            tarball_url: "http://example.com/tarball.tar.gz".into(),
            assets: vec![
                ReleaseAsset {
                    name: "app-linux-x86_64.tar.gz".into(),
                    size: 1000,
                    download_url: "http://example.com/linux-x86_64".into(),
                },
                ReleaseAsset {
                    name: "app-darwin-aarch64.tar.gz".into(),
                    size: 1000,
                    download_url: "http://example.com/darwin-aarch64".into(),
                },
            ],
            ..Default::default()
        };

        // Filters that match nothing in the available assets
        let filters = vec!["*windows*".to_string(), "*arm*".to_string()];

        // --- 1. Check Target Directory ---

        // Directory does not exist: /target
        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| false);

        // Create directory: /target
        runtime
            .expect_create_dir_all()
            .with(eq(target.clone()))
            .returning(|_| Ok(()));

        // --- Execute ---
        let result = ensure_installed_impl(
            &runtime,
            &target,
            &repo,
            &release,
            &HttpDownloader::new(HttpClient::new(Client::new())),
            &MockArchiveExtractor::new(),
            Arc::new(Mutex::new(CleanupContext::new())),
            &filters,
            &[],
        )
        .await;

        // --- Verify Error ---
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("No assets matched the filter patterns"),
            "Expected 'No assets matched' error but got: {}",
            error_msg
        );
        assert!(
            error_msg.contains("*windows*"),
            "Error should mention the filter pattern"
        );
    }

    #[tokio::test]
    async fn test_ensure_installed_filter_without_wildcards_shows_hint() {
        // Test that when a filter without wildcards matches nothing,
        // the error message includes a hint to use wildcards

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup ---
        let target = PathBuf::from("/target"); // Installation target directory
        let repo = RepoId {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        let release = Release {
            tag: "v1.0.0".into(),
            tarball_url: "http://example.com/tarball.tar.gz".into(),
            assets: vec![
                ReleaseAsset {
                    name: "app-linux-x86_64.tar.gz".into(),
                    size: 1000,
                    download_url: "http://example.com/linux-x86_64".into(),
                },
                ReleaseAsset {
                    name: "app-darwin-aarch64.tar.gz".into(),
                    size: 1000,
                    download_url: "http://example.com/darwin-aarch64".into(),
                },
            ],
            ..Default::default()
        };

        // Filter WITHOUT wildcards that matches nothing
        let filters = vec!["linux".to_string()];

        // --- 1. Check Target Directory ---

        // Directory does not exist: /target
        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| false);

        // --- Execute ---
        let result = ensure_installed_impl(
            &runtime,
            &target,
            &repo,
            &release,
            &HttpDownloader::new(HttpClient::new(Client::new())),
            &MockArchiveExtractor::new(),
            Arc::new(Mutex::new(CleanupContext::new())),
            &filters,
            &[
                "ghri".to_string(),
                "install".to_string(),
                "owner/repo".to_string(),
                "--filter".to_string(),
                "linux".to_string(),
            ],
        )
        .await;

        // --- Verify Error with Hint ---
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();

        // Should contain the basic error message
        assert!(
            error_msg.contains("No assets matched the filter patterns"),
            "Expected 'No assets matched' error but got: {}",
            error_msg
        );

        // Should contain the hint about wildcards
        assert!(
            error_msg.contains("Hint:"),
            "Expected hint about wildcards but got: {}",
            error_msg
        );
        assert!(
            error_msg.contains("don't contain wildcards"),
            "Expected mention of missing wildcards but got: {}",
            error_msg
        );
        // Verify the suggested command is built correctly from original_args
        assert!(
            error_msg.contains("ghri install owner/repo --filter \"*linux*\""),
            "Expected suggested command with wildcards but got: {}",
            error_msg
        );
    }

    // --- Tests for binary executable detection (Platform-specific) ---

    #[cfg(target_os = "linux")]
    #[test]
    fn test_is_native_executable_elf_on_linux() {
        // Test that ELF binaries are detected as native on Linux
        // Uses a minimal valid ELF64 header

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/binary");

        // Minimal valid ELF64 header (64 bytes)
        let mut elf_header = vec![
            0x7F, b'E', b'L', b'F', // Magic number
            2,    // 64-bit (EI_CLASS)
            1,    // Little endian (EI_DATA)
            1,    // ELF version (EI_VERSION)
            0,    // OS/ABI (EI_OSABI)
            0, 0, 0, 0, 0, 0, 0, 0, // Padding
            2, 0, // Type: ET_EXEC (executable)
            0x3E, 0, // Machine: x86-64
            1, 0, 0, 0, // Version
        ];
        // Pad to 64 bytes (minimum ELF header size)
        elf_header.resize(64, 0);

        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(move |_| Ok(Box::new(std::io::Cursor::new(elf_header.clone()))));

        assert!(is_native_executable(&runtime, &path));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_is_native_executable_macho_not_native_on_linux() {
        // Test that Mach-O binaries are NOT detected as native on Linux
        // Even though it's a valid binary, it's for macOS, not Linux

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/binary");

        // Minimal Mach-O 64-bit header (32 bytes)
        let macho_header = vec![
            0xCF, 0xFA, 0xED, 0xFE, // Magic: MH_MAGIC_64 (little endian)
            0x0C, 0x00, 0x00, 0x01, // CPU type: ARM64
            0x00, 0x00, 0x00, 0x00, // CPU subtype
            0x02, 0x00, 0x00, 0x00, // File type: MH_EXECUTE
            0x00, 0x00, 0x00, 0x00, // Number of load commands
            0x00, 0x00, 0x00, 0x00, // Size of load commands
            0x00, 0x00, 0x00, 0x00, // Flags
            0x00, 0x00, 0x00, 0x00, // Reserved
        ];

        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(move |_| Ok(Box::new(std::io::Cursor::new(macho_header.clone()))));

        // On Linux, Mach-O should NOT be considered native
        assert!(!is_native_executable(&runtime, &path));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_is_native_executable_macho_64_on_macos() {
        // Test that Mach-O 64-bit binaries are detected as native on macOS
        // Uses a minimal valid Mach-O 64-bit header

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/binary");

        // Minimal Mach-O 64-bit header (32 bytes)
        let macho_header = vec![
            0xCF, 0xFA, 0xED, 0xFE, // Magic: MH_MAGIC_64 (little endian)
            0x0C, 0x00, 0x00, 0x01, // CPU type: ARM64
            0x00, 0x00, 0x00, 0x00, // CPU subtype
            0x02, 0x00, 0x00, 0x00, // File type: MH_EXECUTE
            0x00, 0x00, 0x00, 0x00, // Number of load commands
            0x00, 0x00, 0x00, 0x00, // Size of load commands
            0x00, 0x00, 0x00, 0x00, // Flags
            0x00, 0x00, 0x00, 0x00, // Reserved
        ];

        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(move |_| Ok(Box::new(std::io::Cursor::new(macho_header.clone()))));

        assert!(is_native_executable(&runtime, &path));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_is_native_executable_macho_fat_on_macos() {
        // Test that Mach-O universal (fat) binaries are detected as native on macOS
        // Uses a minimal valid Fat binary header

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/binary");

        // Minimal Fat binary header (8 bytes + arch entries)
        // FAT_MAGIC is 0xCAFEBABE (big endian), with 1 arch entry
        let fat_header = vec![
            0xCA, 0xFE, 0xBA, 0xBE, // Magic: FAT_MAGIC (big endian)
            0x00, 0x00, 0x00, 0x01, // Number of architectures: 1
            0x01, 0x00, 0x00, 0x07, // CPU type (placeholder)
            0x00, 0x00, 0x00, 0x03, // CPU subtype (placeholder)
            0x00, 0x00, 0x10, 0x00, // Offset
            0x00, 0x00, 0x10, 0x00, // Size
            0x00, 0x00, 0x00, 0x0C, // Align
        ];

        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(move |_| Ok(Box::new(std::io::Cursor::new(fat_header.clone()))));

        assert!(is_native_executable(&runtime, &path));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_is_native_executable_elf_not_native_on_macos() {
        // Test that ELF binaries are NOT detected as native on macOS
        // Even though it's a valid binary, it's for Linux, not macOS

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/binary");

        // Minimal valid ELF64 header (64 bytes)
        let mut elf_header = vec![
            0x7F, b'E', b'L', b'F', // Magic number
            2,    // 64-bit (EI_CLASS)
            1,    // Little endian (EI_DATA)
            1,    // ELF version (EI_VERSION)
            0,    // OS/ABI (EI_OSABI)
            0, 0, 0, 0, 0, 0, 0, 0, // Padding
            2, 0, // Type: ET_EXEC (executable)
            0x3E, 0, // Machine: x86-64
            1, 0, 0, 0, // Version
        ];
        // Pad to 64 bytes (minimum ELF header size)
        elf_header.resize(64, 0);

        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(move |_| Ok(Box::new(std::io::Cursor::new(elf_header.clone()))));

        // On macOS, ELF should NOT be considered native
        assert!(!is_native_executable(&runtime, &path));
    }

    #[cfg(unix)]
    #[test]
    fn test_is_native_executable_script() {
        // Test that scripts (shebang) are NOT detected as native executables
        // Script starts with "#!" which should not be treated as native binary

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/script.sh");

        // Shebang for shell script
        let script_header = b"#!/bin/bash\necho hello".to_vec();
        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(move |_| Ok(Box::new(std::io::Cursor::new(script_header.clone()))));

        assert!(!is_native_executable(&runtime, &path));
    }

    #[cfg(unix)]
    #[test]
    fn test_is_native_executable_text_file() {
        // Test that plain text files are NOT detected as native executables

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/readme.txt");

        let text_content = b"This is a readme file".to_vec();
        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(move |_| Ok(Box::new(std::io::Cursor::new(text_content.clone()))));

        assert!(!is_native_executable(&runtime, &path));
    }

    #[cfg(unix)]
    #[test]
    fn test_is_native_executable_empty_file() {
        // Test that empty files are NOT detected as native executables

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/empty");

        // Empty file (read_exact will fail)
        let empty_content: Vec<u8> = vec![];
        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(move |_| Ok(Box::new(std::io::Cursor::new(empty_content.clone()))));

        assert!(!is_native_executable(&runtime, &path));
    }

    #[cfg(unix)]
    #[test]
    fn test_is_native_executable_file_not_found() {
        // Test that non-existent files return false (not an error)

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/nonexistent");

        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(|_| Err(anyhow::anyhow!("file not found")));

        assert!(!is_native_executable(&runtime, &path));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_set_executable_if_binary_sets_permission_for_elf_on_linux() {
        // Test that set_executable_if_binary sets 0o755 for ELF binaries on Linux

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/binary");

        // Minimal valid ELF64 header (64 bytes)
        let mut elf_header = vec![
            0x7F, b'E', b'L', b'F', // Magic number
            2,    // 64-bit (EI_CLASS)
            1,    // Little endian (EI_DATA)
            1,    // ELF version (EI_VERSION)
            0,    // OS/ABI (EI_OSABI)
            0, 0, 0, 0, 0, 0, 0, 0, // Padding
            2, 0, // Type: ET_EXEC (executable)
            0x3E, 0, // Machine: x86-64
            1, 0, 0, 0, // Version
        ];
        // Pad to 64 bytes (minimum ELF header size)
        elf_header.resize(64, 0);

        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(move |_| Ok(Box::new(std::io::Cursor::new(elf_header.clone()))));

        // Expect set_permissions to be called with 0o755
        runtime
            .expect_set_permissions()
            .with(eq(path.clone()), eq(0o755))
            .times(1)
            .returning(|_, _| Ok(()));

        let result = set_executable_if_binary(&runtime, &path);
        assert!(result.is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_set_executable_if_binary_sets_permission_for_macho_on_macos() {
        // Test that set_executable_if_binary sets 0o755 for Mach-O binaries on macOS

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/binary");

        // Minimal Mach-O 64-bit header (32 bytes)
        let macho_header = vec![
            0xCF, 0xFA, 0xED, 0xFE, // Magic: MH_MAGIC_64 (little endian)
            0x0C, 0x00, 0x00, 0x01, // CPU type: ARM64
            0x00, 0x00, 0x00, 0x00, // CPU subtype
            0x02, 0x00, 0x00, 0x00, // File type: MH_EXECUTE
            0x00, 0x00, 0x00, 0x00, // Number of load commands
            0x00, 0x00, 0x00, 0x00, // Size of load commands
            0x00, 0x00, 0x00, 0x00, // Flags
            0x00, 0x00, 0x00, 0x00, // Reserved
        ];

        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(move |_| Ok(Box::new(std::io::Cursor::new(macho_header.clone()))));

        // Expect set_permissions to be called with 0o755
        runtime
            .expect_set_permissions()
            .with(eq(path.clone()), eq(0o755))
            .times(1)
            .returning(|_, _| Ok(()));

        let result = set_executable_if_binary(&runtime, &path);
        assert!(result.is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_set_executable_if_binary_skips_elf_on_macos() {
        // Test that set_executable_if_binary does NOT set permission for ELF binaries on macOS
        // ELF is not native to macOS, so no executable permission should be set

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/binary");

        // Minimal valid ELF64 header (64 bytes)
        let mut elf_header = vec![
            0x7F, b'E', b'L', b'F', // Magic number
            2,    // 64-bit (EI_CLASS)
            1,    // Little endian (EI_DATA)
            1,    // ELF version (EI_VERSION)
            0,    // OS/ABI (EI_OSABI)
            0, 0, 0, 0, 0, 0, 0, 0, // Padding
            2, 0, // Type: ET_EXEC (executable)
            0x3E, 0, // Machine: x86-64
            1, 0, 0, 0, // Version
        ];
        // Pad to 64 bytes (minimum ELF header size)
        elf_header.resize(64, 0);

        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(move |_| Ok(Box::new(std::io::Cursor::new(elf_header.clone()))));

        // set_permissions should NOT be called for ELF on macOS
        // (MockRuntime will fail if an unexpected call is made)

        let result = set_executable_if_binary(&runtime, &path);
        assert!(result.is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_set_executable_if_binary_skips_macho_on_linux() {
        // Test that set_executable_if_binary does NOT set permission for Mach-O binaries on Linux
        // Mach-O is not native to Linux, so no executable permission should be set

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/binary");

        // Minimal Mach-O 64-bit header (32 bytes)
        let macho_header = vec![
            0xCF, 0xFA, 0xED, 0xFE, // Magic: MH_MAGIC_64 (little endian)
            0x0C, 0x00, 0x00, 0x01, // CPU type: ARM64
            0x00, 0x00, 0x00, 0x00, // CPU subtype
            0x02, 0x00, 0x00, 0x00, // File type: MH_EXECUTE
            0x00, 0x00, 0x00, 0x00, // Number of load commands
            0x00, 0x00, 0x00, 0x00, // Size of load commands
            0x00, 0x00, 0x00, 0x00, // Flags
            0x00, 0x00, 0x00, 0x00, // Reserved
        ];

        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(move |_| Ok(Box::new(std::io::Cursor::new(macho_header.clone()))));

        // set_permissions should NOT be called for Mach-O on Linux
        // (MockRuntime will fail if an unexpected call is made)

        let result = set_executable_if_binary(&runtime, &path);
        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_set_executable_if_binary_skips_text_file() {
        // Test that set_executable_if_binary does NOT set permission for text files

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let path = PathBuf::from("/test/readme.txt");

        let text_content = b"plain text".to_vec();
        runtime
            .expect_open()
            .with(eq(path.clone()))
            .returning(move |_| Ok(Box::new(std::io::Cursor::new(text_content.clone()))));

        // set_permissions should NOT be called for text files
        // (MockRuntime will fail if an unexpected call is made)

        let result = set_executable_if_binary(&runtime, &path);
        assert!(result.is_ok());
    }
}
