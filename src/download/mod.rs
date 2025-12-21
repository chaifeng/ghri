use crate::retry::{check_retryable, MAX_RETRIES};
use crate::runtime::Runtime;
use anyhow::{Context, Result};
use log::{debug, info, warn};
use reqwest::Client;
use std::io::Write;
use std::path::Path;

/// Downloads a file from a URL to a temporary path with retry support.
#[tracing::instrument(skip(runtime, temp_path, client))]
pub async fn download_file<R: Runtime>(
    runtime: &R,
    url: &str,
    temp_path: &Path,
    client: &Client,
) -> Result<()> {
    info!("Downloading file from {}...", url);

    let mut last_error = None;

    for attempt in 1..=MAX_RETRIES {
        match download_file_once(runtime, url, temp_path, client).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                // Check if this is a non-retryable error
                if e.downcast_ref::<crate::retry::NonRetryableError>()
                    .is_some()
                {
                    return Err(e);
                }

                let error_str = e.to_string();
                let is_network_error = error_str.contains("connection")
                    || error_str.contains("timeout")
                    || error_str.contains("reset")
                    || error_str.contains("broken pipe")
                    || error_str.contains("dns")
                    || error_str.contains("resolve")
                    || error_str.contains("chunk");

                if is_network_error && attempt < MAX_RETRIES {
                    warn!(
                        "Download attempt {}/{} failed ({}), retrying...",
                        attempt, MAX_RETRIES, error_str
                    );
                    last_error = Some(e);
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                } else {
                    return Err(e);
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Download failed after {} attempts", MAX_RETRIES)))
}

/// Single download attempt without retry.
async fn download_file_once<R: Runtime>(
    runtime: &R,
    url: &str,
    temp_path: &Path,
    client: &Client,
) -> Result<()> {
    let response = client
        .get(url)
        .send()
        .await
        .context("Failed to start download request")?;

    let mut response = response.error_for_status().map_err(check_retryable)?;

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

        let mut runtime = MockRuntime::new();
        runtime.expect_create_file()
            .with(mockall::predicate::eq(Path::new("test.file").to_path_buf()))
            .returning(|_| Ok(Box::new(std::io::sink())));

        let temp_path = Path::new("test.file");
        let client = Client::new();

        let result =
            download_file(&runtime, &format!("{}/test.file", url), temp_path, &client).await;

        mock.assert_async().await;
        assert!(result.is_ok());
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

        let runtime = MockRuntime::new(); // No expectations = strict (panic if called)
        let temp_path = Path::new("test.file");
        let client = Client::new();

        let result =
            download_file(&runtime, &format!("{}/test.file", url), temp_path, &client).await;

        mock.assert_async().await;
        assert!(result.is_err());
    }
}
