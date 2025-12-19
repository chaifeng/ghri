use crate::runtime::Runtime;
use anyhow::{Context, Result};
use log::{debug, info};
use reqwest::Client;
use std::io::Write;
use std::path::Path;

/// Downloads a file from a URL to a temporary path.
pub async fn download_file<R: Runtime>(
    runtime: &R,
    url: &str,
    temp_path: &Path,
    client: &Client,
) -> Result<()> {
    info!("Downloading file from {}...", url);
    let mut response = client
        .get(url)
        .send()
        .await
        .context("Failed to start download request")?
        .error_for_status()
        .context("Download request failed with an error status")?;

    let mut temp_file = runtime
        .create_file(temp_path)
        .with_context(|| format!("Failed to create temporary file at {:?}", temp_path))?;

    let mut downloaded_bytes = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .context("Failed to read chunk from download stream")?
    {
        temp_file
            .write_all(&chunk)
            .context("Failed to write chunk to temporary file")?;
        downloaded_bytes += chunk.len();
    }
    debug!(
        "Downloaded {:.2} MB",
        downloaded_bytes as f64 / (1024.0 * 1024.0)
    );
    info!("Download complete.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;

    #[tokio::test]
    async fn test_download_file() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mock = server
            .mock("GET", "/test.file")
            .with_status(200)
            .with_body("test content")
            .create_async()
            .await;

        let runtime = MockRuntime::new();
        let temp_path = Path::new("test.file");
        let client = Client::new();

        let result =
            download_file(&runtime, &format!("{}/test.file", url), temp_path, &client).await;

        mock.assert_async().await;
        assert!(result.is_ok());
        assert_eq!(runtime.read_to_string(temp_path).unwrap(), "test content");
    }

    #[tokio::test]
    async fn test_download_file_not_found() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mock = server
            .mock("GET", "/test.file")
            .with_status(404)
            .create_async()
            .await;

        let runtime = MockRuntime::new();
        let temp_path = Path::new("test.file");
        let client = Client::new();

        let result =
            download_file(&runtime, &format!("{}/test.file", url), temp_path, &client).await;

        mock.assert_async().await;
        assert!(result.is_err());
    }
}
