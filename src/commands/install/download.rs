use anyhow::{Context, Result};
use log::{debug, info};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::{
    archive::Extractor,
    cleanup::CleanupContext,
    download::download_file,
    github::{GitHubRepo, Release},
    http::HttpClient,
    runtime::Runtime,
};

#[tracing::instrument(skip(runtime, target_dir, repo, release, http_client, extractor, cleanup_ctx))]
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

    let temp_dir = std::env::temp_dir();
    let temp_file_path = temp_dir.join(format!("{}-{}.tar.gz", repo.repo, release.tag_name));

    println!(" downloading {} {}", &repo, release.tag_name);
    if let Err(e) = download_file(runtime, &release.tarball_url, &temp_file_path, http_client).await {
        // Clean up target directory on download failure
        debug!("Download failed, cleaning up target directory: {:?}", target_dir);
        let _ = runtime.remove_dir_all(target_dir);
        return Err(e);
    }

    // Register temp file for cleanup (after download succeeds)
    {
        let mut ctx = cleanup_ctx.lock().unwrap();
        ctx.add(temp_file_path.clone());
    }

    println!("  installing {} {}", &repo, release.tag_name);
    if let Err(e) = extractor.extract_with_cleanup(runtime, &temp_file_path, target_dir, Arc::clone(&cleanup_ctx)) {
        // Clean up target directory and temp file on extraction failure
        debug!("Extraction failed, cleaning up target directory: {:?}", target_dir);
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

    // Installation succeeded, remove target_dir from cleanup list
    {
        let mut ctx = cleanup_ctx.lock().unwrap();
        ctx.remove(target_dir);
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
        let target = PathBuf::from("/target");                    // Installation target directory
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
        let _m = server.mock("GET", "/download").with_status(200).with_body("data").create();

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
        runtime
            .expect_remove_file()
            .times(1)
            .returning(|_| Ok(()));

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
        assert!(result.unwrap_err().to_string().contains("extraction failed"));
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
}
