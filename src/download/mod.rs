use anyhow::{Context, Result};
use log::{debug, info};
use reqwest::Client;
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// Downloads a file from a URL to a temporary path.
pub async fn download_file(url: &str, temp_path: &Path, client: &Client) -> Result<()> {
    info!("Downloading file from {}...", url);
    let mut response = client
        .get(url)
        .send()
        .await
        .context("Failed to start download request")?
        .error_for_status()
        .context("Download request failed with an error status")?;

    let mut temp_file = File::create(temp_path)
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
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_download_file() {
        let mut server = mockito::Server::new();
        let url = server.url();

        let mock = server
            .mock("GET", "/test.file")
            .with_status(200)
            .with_body("test content")
            .create();

        let dir = tempdir().unwrap();
        let temp_path = dir.path().join("test.file");

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let client = Client::new();
            download_file(&format!("{}/test.file", url), &temp_path, &client).await
        })
        .unwrap();

        mock.assert();
        assert_eq!(fs::read_to_string(temp_path).unwrap(), "test content");
    }

    #[test]
    fn test_download_file_not_found() {
        let mut server = mockito::Server::new();
        let url = server.url();

        let mock = server.mock("GET", "/test.file").with_status(404).create();

        let dir = tempdir().unwrap();
        let temp_path = dir.path().join("test.file");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            let client = Client::new();
            download_file(&format!("{}/test.file", url), &temp_path, &client).await
        });

        mock.assert();
        assert!(result.is_err());
    }
}
