use crate::http::HttpClient;
use crate::runtime::Runtime;
use anyhow::{Context, Result};
use log::info;
use std::path::Path;

/// Downloads a file from a URL to a temporary path with retry support.
#[tracing::instrument(skip(runtime, temp_path, http_client))]
pub async fn download_file<R: Runtime>(
    runtime: &R,
    url: &str,
    temp_path: &Path,
    http_client: &HttpClient,
) -> Result<()> {
    info!("Downloading file from {}...", url);

    let temp_path = temp_path.to_path_buf();
    http_client
        .download_file(url, || {
            runtime
                .create_file(&temp_path)
                .with_context(|| format!("Failed to create temporary file at {:?}", temp_path))
        })
        .await?;

    info!("Download complete.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use reqwest::Client;

    #[tokio::test]
    async fn test_download_file() {
        // Test successful file download

        // --- Setup Mock Server ---
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        // Server returns 200 OK with content
        let mock = server
            .mock("GET", "/test.file")
            .with_status(200)
            .with_body("test content")
            .create_async()
            .await;

        // --- Setup Runtime ---
        let mut runtime = MockRuntime::new();

        // Create file: test.file -> returns sink (discards content)
        runtime
            .expect_create_file()
            .with(mockall::predicate::eq(Path::new("test.file").to_path_buf()))
            .returning(|_| Ok(Box::new(std::io::sink())));

        // --- Execute ---
        let temp_path = Path::new("test.file");
        let http_client = HttpClient::new(Client::new());

        let result = download_file(
            &runtime,
            &format!("{}/test.file", url),
            temp_path,
            &http_client,
        )
        .await;

        // --- Verify ---
        mock.assert_async().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_download_file_not_found() {
        // Test that download fails when server returns 404

        // --- Setup Mock Server ---
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        // Server returns 404 Not Found
        let mock = server
            .mock("GET", "/test.file")
            .with_status(404)
            .create_async()
            .await;

        // --- Setup Runtime ---
        // No expectations = strict mode (panics if any method called)
        let runtime = MockRuntime::new();

        // --- Execute ---
        let temp_path = Path::new("test.file");
        let http_client = HttpClient::new(Client::new());

        let result = download_file(
            &runtime,
            &format!("{}/test.file", url),
            temp_path,
            &http_client,
        )
        .await;

        // --- Verify ---
        mock.assert_async().await;
        assert!(result.is_err());
    }
}
