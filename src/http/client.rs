//! HTTP client with built-in retry logic and error handling.

use anyhow::{Context, Result};
use log::{debug, warn};
use reqwest::Client;
use serde::de::DeserializeOwned;
use std::io::Write;

use super::retry::{MAX_RETRIES, NonRetryableError, RETRY_DELAY_MS, check_retryable};

/// HTTP client with built-in retry logic for network operations.
#[derive(Clone)]
pub struct HttpClient {
    client: Client,
}

impl HttpClient {
    /// Creates a new HTTP client wrapping the given reqwest Client.
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Returns a reference to the underlying reqwest Client.
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// Performs a GET request and deserializes the JSON response.
    /// Automatically retries on transient errors.
    #[tracing::instrument(skip(self))]
    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        debug!("GET JSON from {}...", url);

        self.with_retry("GET JSON", || async {
            let response = self
                .client
                .get(url)
                .send()
                .await
                .context("Failed to send request")?;

            let response = response.error_for_status().map_err(check_retryable)?;

            let result = response
                .json::<T>()
                .await
                .context("Failed to parse JSON response")?;

            Ok(result)
        })
        .await
    }

    /// Performs a GET request with query parameters and deserializes the JSON response.
    /// Automatically retries on transient errors.
    #[tracing::instrument(skip(self, query))]
    pub async fn get_json_with_query<T: DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        debug!("GET JSON from {} with query {:?}...", url, query);

        self.with_retry("GET JSON with query", || async {
            let response = self
                .client
                .get(url)
                .query(query)
                .send()
                .await
                .context("Failed to send request")?;

            let response = response.error_for_status().map_err(check_retryable)?;

            let result = response
                .json::<T>()
                .await
                .context("Failed to parse JSON response")?;

            Ok(result)
        })
        .await
    }

    /// Downloads a file from a URL to the specified path.
    /// Automatically retries on transient errors.
    /// Uses a writer function to allow for custom file creation (e.g., via Runtime).
    #[tracing::instrument(skip(self, create_writer))]
    pub async fn download_file<W, F>(&self, url: &str, create_writer: F) -> Result<u64>
    where
        W: Write,
        F: Fn() -> Result<W>,
    {
        debug!("Downloading file from {}...", url);

        let mut last_error = None;

        for attempt in 1..=MAX_RETRIES {
            match self.download_file_once(url, &create_writer).await {
                Ok(bytes) => return Ok(bytes),
                Err(e) => {
                    // Check if this is a non-retryable error
                    if e.downcast_ref::<NonRetryableError>().is_some() {
                        return Err(e);
                    }

                    if attempt < MAX_RETRIES {
                        warn!(
                            "Download attempt {}/{} failed ({}), retrying...",
                            attempt, MAX_RETRIES, e
                        );
                        last_error = Some(e);
                        tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("Download failed after {} attempts", MAX_RETRIES)))
    }

    /// Single download attempt without retry.
    async fn download_file_once<W, F>(&self, url: &str, create_writer: &F) -> Result<u64>
    where
        W: Write,
        F: Fn() -> Result<W>,
    {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Failed to start download request")?;

        let mut response = response.error_for_status().map_err(check_retryable)?;

        let mut writer = create_writer()?;
        let mut downloaded_bytes: u64 = 0;

        while let Some(chunk) = response
            .chunk()
            .await
            .context("Failed to read chunk from download stream")?
        {
            writer
                .write_all(&chunk)
                .context("Failed to write chunk to file")?;
            downloaded_bytes += chunk.len() as u64;
        }

        debug!(
            "Downloaded {:.2} MB",
            downloaded_bytes as f64 / (1024.0 * 1024.0)
        );

        Ok(downloaded_bytes)
    }

    /// Executes an async operation with retry logic.
    async fn with_retry<F, Fut, T>(&self, operation_name: &str, operation: F) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut last_error = None;

        for attempt in 1..=MAX_RETRIES {
            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if !is_retryable_error(&e) {
                        debug!("{}: non-retryable error: {}", operation_name, e);
                        return Err(e);
                    }

                    if attempt < MAX_RETRIES {
                        warn!(
                            "{}: attempt {}/{} failed ({}), retrying in {}ms...",
                            operation_name, attempt, MAX_RETRIES, e, RETRY_DELAY_MS
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    }
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!("{}: failed after {} attempts", operation_name, MAX_RETRIES)
        }))
    }
}

/// Checks if an anyhow::Error is retryable based on its content.
fn is_retryable_error(e: &anyhow::Error) -> bool {
    // Non-retryable errors should not be retried
    if e.downcast_ref::<NonRetryableError>().is_some() {
        return false;
    }

    // Retry everything else that isn't explicitly non-retryable
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_json_success() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mock = server
            .mock("GET", "/test")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"name": "test", "value": 42}"#)
            .create_async()
            .await;

        let client = HttpClient::new(Client::new());

        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct TestResponse {
            name: String,
            value: i32,
        }

        let result: TestResponse = client.get_json(&format!("{}/test", url)).await.unwrap();

        mock.assert_async().await;
        assert_eq!(result.name, "test");
        assert_eq!(result.value, 42);
    }

    #[tokio::test]
    async fn test_get_json_not_found() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mock = server
            .mock("GET", "/test")
            .with_status(404)
            .create_async()
            .await;

        let client = HttpClient::new(Client::new());

        let result: Result<serde_json::Value> = client.get_json(&format!("{}/test", url)).await;

        mock.assert_async().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_json_with_query_success() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mock = server
            .mock("GET", "/test?page=1&per_page=10")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"["item1", "item2"]"#)
            .create_async()
            .await;

        let client = HttpClient::new(Client::new());
        let result: Vec<String> = client
            .get_json_with_query(
                &format!("{}/test", url),
                &[("page", "1"), ("per_page", "10")],
            )
            .await
            .unwrap();

        mock.assert_async().await;
        assert_eq!(result, vec!["item1", "item2"]);
    }

    #[tokio::test]
    async fn test_download_file_success() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mock = server
            .mock("GET", "/file.txt")
            .with_status(200)
            .with_body("test content")
            .create_async()
            .await;

        let client = HttpClient::new(Client::new());
        let bytes = client
            .download_file(&format!("{}/file.txt", url), || Ok(std::io::sink()))
            .await
            .unwrap();

        mock.assert_async().await;
        assert_eq!(bytes, 12); // "test content" is 12 bytes
    }

    #[tokio::test]
    async fn test_download_file_not_found() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mock = server
            .mock("GET", "/file.txt")
            .with_status(404)
            .create_async()
            .await;

        let client = HttpClient::new(Client::new());
        let result = client
            .download_file(&format!("{}/file.txt", url), || Ok(std::io::sink()))
            .await;

        mock.assert_async().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_is_retryable_error_timeout() {
        let err = anyhow::anyhow!("connection timeout");
        assert!(is_retryable_error(&err));
    }

    #[test]
    fn test_is_retryable_error_dns() {
        let err = anyhow::anyhow!("dns lookup failed");
        assert!(is_retryable_error(&err));
    }

    #[test]
    fn test_is_retryable_error_resolve() {
        let err = anyhow::anyhow!("failed to resolve host");
        assert!(is_retryable_error(&err));
    }

    #[test]
    fn test_is_retryable_error_broken_pipe() {
        let err = anyhow::anyhow!("broken pipe");
        assert!(is_retryable_error(&err));
    }

    #[test]
    fn test_is_retryable_error() {
        // Non-retryable error
        let err = anyhow::Error::from(NonRetryableError::NotFound("test".to_string()));
        assert!(!is_retryable_error(&err));

        // Network-like error (retryable)
        let err = anyhow::anyhow!("connection reset by peer");
        assert!(is_retryable_error(&err));

        // Generic error (now retryable)
        let err = anyhow::anyhow!("some other error");
        assert!(is_retryable_error(&err));
    }

    #[tokio::test]
    async fn test_with_retry_success() {
        let client = HttpClient::new(Client::new());
        let result = client
            .with_retry("test", || async { Ok::<_, anyhow::Error>("success") })
            .await;
        assert_eq!(result.unwrap(), "success");
    }

    #[tokio::test]
    async fn test_with_retry_immediate_failure_on_non_retryable() {
        let client = HttpClient::new(Client::new());
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let result = client
            .with_retry("test", || {
                let count = call_count_clone.clone();
                async move {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err::<(), _>(anyhow::Error::from(NonRetryableError::NotFound(
                        "not found".to_string(),
                    )))
                }
            })
            .await;

        assert!(result.is_err());
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_with_retry_retries_on_network_error() {
        let client = HttpClient::new(Client::new());
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let result = client
            .with_retry("test", || {
                let count = call_count_clone.clone();
                async move {
                    let current = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if current < 2 {
                        Err::<&str, _>(anyhow::anyhow!("connection reset"))
                    } else {
                        Ok("success after retries")
                    }
                }
            })
            .await;

        assert_eq!(result.unwrap(), "success after retries");
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_with_retry_exhausts_retries() {
        let client = HttpClient::new(Client::new());
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let result = client
            .with_retry("test", || {
                let count = call_count_clone.clone();
                async move {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err::<(), _>(anyhow::anyhow!("connection timeout"))
                }
            })
            .await;

        assert!(result.is_err());
        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::SeqCst),
            MAX_RETRIES
        );
    }
}
