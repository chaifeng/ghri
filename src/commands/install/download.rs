use anyhow::{Context, Result};
use log::{debug, info};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{
    archive::Extractor,
    cleanup::CleanupContext,
    download::download_file,
    github::{GitHubRepo, Release},
    http::HttpClient,
    runtime::Runtime,
};

#[tracing::instrument(skip(
    runtime,
    target_dir,
    repo,
    release,
    http_client,
    extractor,
    cleanup_ctx
))]
pub(crate) async fn ensure_installed<R: Runtime + 'static, E: Extractor>(
    runtime: &R,
    target_dir: &Path,
    repo: &GitHubRepo,
    release: &Release,
    http_client: &HttpClient,
    extractor: &E,
    cleanup_ctx: Arc<Mutex<CleanupContext>>,
) -> Result<()> {
    if runtime.exists(target_dir) {
        info!(
            "Directory {:?} already exists. Skipping download and extraction.",
            target_dir
        );
        return Ok(());
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

    // Choose download strategy based on assets availability
    if release.assets.is_empty() {
        // No assets: download source tarball
        download_and_extract_tarball(
            runtime,
            target_dir,
            repo,
            release,
            http_client,
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
            release,
            http_client,
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

/// Download source tarball (when no assets available)
/// Since it's a single file that is an archive, extract it
async fn download_and_extract_tarball<R: Runtime + 'static, E: Extractor>(
    runtime: &R,
    target_dir: &Path,
    repo: &GitHubRepo,
    release: &Release,
    http_client: &HttpClient,
    extractor: &E,
    cleanup_ctx: Arc<Mutex<CleanupContext>>,
) -> Result<()> {
    let temp_dir = std::env::temp_dir();
    let temp_file_path = temp_dir.join(format!("{}-{}.tar.gz", repo.repo, release.tag_name));

    println!(" downloading {} {} (source)", &repo, release.tag_name);
    println!(
        " downloading {} {} -> {}",
        &repo, release.tag_name, release.tarball_url
    );
    if let Err(e) = download_file(runtime, &release.tarball_url, &temp_file_path, http_client).await
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
    println!("  installing {} {}", &repo, release.tag_name);
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

/// Check if a filename represents an archive that can be extracted
fn is_archive(name: &str) -> bool {
    let name_lower = name.to_lowercase();
    name_lower.ends_with(".tar.gz")
        || name_lower.ends_with(".tgz")
        || name_lower.ends_with(".tar.xz")
        || name_lower.ends_with(".tar.bz2")
        || name_lower.ends_with(".zip")
}

/// Download all release assets (when assets are available)
/// If only one file is downloaded and it's an archive, extract it.
/// If multiple files are downloaded, keep them as-is without extraction.
async fn download_all_assets<R: Runtime + 'static, E: Extractor>(
    runtime: &R,
    target_dir: &Path,
    repo: &GitHubRepo,
    release: &Release,
    http_client: &HttpClient,
    extractor: &E,
    cleanup_ctx: Arc<Mutex<CleanupContext>>,
) -> Result<()> {
    let temp_dir = std::env::temp_dir();
    let mut temp_files: Vec<PathBuf> = Vec::new();

    let assets_count: usize = release.assets.len();
    println!(
        " downloading {} {} ({} assets)",
        &repo, release.tag_name, assets_count
    );

    // Download all assets
    let mut asset_index = 0;
    for asset in &release.assets {
        asset_index += 1;
        let temp_file_path = temp_dir.join(format!(
            "{}-{}-{}",
            repo.repo, release.tag_name, &asset.name
        ));

        debug!(
            "Downloading asset: {}({}) -> {:?}",
            &asset.name, &asset.browser_download_url, &temp_file_path
        );
        println!(
            " downloading {} {} ({}/{} assets) -> {}",
            &repo, release.tag_name, asset_index, assets_count, &asset.browser_download_url
        );
        if let Err(e) = download_file(
            runtime,
            &asset.browser_download_url,
            &temp_file_path,
            http_client,
        )
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

    println!("  installing {} {}", &repo, release.tag_name);

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
    use crate::archive::MockExtractor;
    use crate::runtime::MockRuntime;
    use mockall::predicate::*;
    use reqwest::Client;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_ensure_installed_creates_dir_and_extracts() {
        // Test successful installation: creates directory, downloads, and extracts

        let mut runtime = MockRuntime::new();

        // --- Setup ---
        let target = PathBuf::from("/target"); // Installation target directory
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag_name: "v1".into(),
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

        let mut extractor = MockExtractor::new();
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
        let http_client = HttpClient::new(Client::new());
        ensure_installed(
            &runtime,
            &target,
            &repo,
            &release_with_url,
            &http_client,
            &extractor,
            cleanup_ctx,
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

        // --- Setup ---
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag_name: "v1".into(),
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

        let mut extractor = MockExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        // --- Execute & Verify ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let http_client = HttpClient::new(Client::new());
        let result = ensure_installed(
            &runtime,
            &target_dir,
            &repo,
            &release,
            &http_client,
            &extractor,
            cleanup_ctx,
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

        // --- Setup ---
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag_name: "v1".into(),
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

        let extractor = MockExtractor::new();

        // --- Execute & Verify ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let http_client = HttpClient::new(Client::new());
        let result = ensure_installed(
            &runtime,
            &target_dir,
            &repo,
            &release,
            &http_client,
            &extractor,
            cleanup_ctx,
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

        // --- Setup ---
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag_name: "v1".into(),
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

        let mut extractor = MockExtractor::new();
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
        let http_client = HttpClient::new(Client::new());
        let result = ensure_installed(
            &runtime,
            &target_dir,
            &repo,
            &release,
            &http_client,
            &extractor,
            cleanup_ctx,
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
        let http_client = HttpClient::new(Client::new());
        let result = ensure_installed(
            &runtime,
            &target,
            &GitHubRepo {
                owner: "o".into(),
                repo: "r".into(),
            },
            &Release::default(),
            &http_client,
            &MockExtractor::new(),
            cleanup_ctx,
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

        // --- Setup ---
        let target = PathBuf::from("/target");
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };

        // Release with 2 assets (archive + text file)
        // Even though one is an archive, since there are multiple files, NONE are extracted
        let release = Release {
            tag_name: "v1".into(),
            tarball_url: format!("{}/tarball", url), // Should NOT be downloaded
            assets: vec![
                crate::github::ReleaseAsset {
                    name: "app-linux-x86_64.tar.gz".into(),
                    size: 1000,
                    browser_download_url: format!("{}/asset1.tar.gz", url),
                },
                crate::github::ReleaseAsset {
                    name: "checksums.txt".into(),
                    size: 100,
                    browser_download_url: format!("{}/checksums.txt", url),
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
        let extractor = MockExtractor::new();

        // Both files are copied directly to target directory
        // Copy: /tmp/r-v1-app-linux-x86_64.tar.gz -> /target/app-linux-x86_64.tar.gz
        // Copy: /tmp/r-v1-checksums.txt -> /target/checksums.txt
        runtime.expect_copy().times(2).returning(|_, _| Ok(100));

        // --- 5. Cleanup Temp Files ---

        // Remove both temp files after processing
        runtime.expect_remove_file().times(2).returning(|_| Ok(()));

        // --- Execute ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let http_client = HttpClient::new(Client::new());
        let result = ensure_installed(
            &runtime,
            &target,
            &repo,
            &release,
            &http_client,
            &extractor,
            cleanup_ctx,
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

        // --- Setup ---
        let target = PathBuf::from("/target");
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };

        // Release with single archive asset
        let release = Release {
            tag_name: "v1".into(),
            tarball_url: format!("{}/tarball", url), // Should NOT be downloaded
            assets: vec![crate::github::ReleaseAsset {
                name: "app-linux-x86_64.tar.gz".into(),
                size: 1000,
                browser_download_url: format!("{}/asset1.tar.gz", url),
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

        let mut extractor = MockExtractor::new();
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
        let http_client = HttpClient::new(Client::new());
        let result = ensure_installed(
            &runtime,
            &target,
            &repo,
            &release,
            &http_client,
            &extractor,
            cleanup_ctx,
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

        // --- Setup ---
        let target = PathBuf::from("/target");
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };

        // Release with single non-archive asset (a binary)
        let release = Release {
            tag_name: "v1".into(),
            tarball_url: format!("{}/tarball", url), // Should NOT be downloaded
            assets: vec![crate::github::ReleaseAsset {
                name: "app-linux-x86_64".into(), // No archive extension
                size: 1000,
                browser_download_url: format!("{}/binary", url),
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
        let extractor = MockExtractor::new();

        // Copy: /tmp/r-v1-app-linux-x86_64 -> /target/app-linux-x86_64
        runtime.expect_copy().times(1).returning(|_, _| Ok(100));

        // --- 5. Cleanup Temp File ---

        // Remove temp file after copy
        runtime.expect_remove_file().times(1).returning(|_| Ok(()));

        // --- Execute ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let http_client = HttpClient::new(Client::new());
        let result = ensure_installed(
            &runtime,
            &target,
            &repo,
            &release,
            &http_client,
            &extractor,
            cleanup_ctx,
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

        // --- Setup ---
        let target = PathBuf::from("/target");
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };

        // Release with NO assets (empty vec)
        let release = Release {
            tag_name: "v1".into(),
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

        let mut extractor = MockExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .times(1)
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        // --- 5. Cleanup Temp File ---

        runtime.expect_remove_file().times(1).returning(|_| Ok(()));

        // --- Execute ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let http_client = HttpClient::new(Client::new());
        let result = ensure_installed(
            &runtime,
            &target,
            &repo,
            &release,
            &http_client,
            &extractor,
            cleanup_ctx,
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

        // --- Setup ---
        let target = PathBuf::from("/target");
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };

        // Release with 2 assets
        let release = Release {
            tag_name: "v1".into(),
            tarball_url: format!("{}/tarball", url),
            assets: vec![
                crate::github::ReleaseAsset {
                    name: "asset1.tar.gz".into(),
                    size: 1000,
                    browser_download_url: format!("{}/asset1.tar.gz", url),
                },
                crate::github::ReleaseAsset {
                    name: "asset2.tar.gz".into(),
                    size: 2000,
                    browser_download_url: format!("{}/asset2.tar.gz", url), // This will fail
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

        let extractor = MockExtractor::new();

        // --- Execute & Verify ---

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let http_client = HttpClient::new(Client::new());
        let result = ensure_installed(
            &runtime,
            &target,
            &repo,
            &release,
            &http_client,
            &extractor,
            cleanup_ctx,
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
}
