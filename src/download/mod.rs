use crate::http::HttpClient;
use crate::runtime::Runtime;
use anyhow::{Context, Result};
use log::info;
use std::path::Path;

/// Trait for downloading files from URLs.
/// Abstracts the download logic to allow for different implementations (HTTP, mock, etc.)
pub trait Downloader: Send + Sync {
    /// Downloads a file from a URL to the specified path.
    fn download<'a, R: Runtime + 'a>(
        &'a self,
        runtime: &'a R,
        url: &'a str,
        dest: &'a Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>;
}

/// HTTP-based downloader implementation using HttpClient.
pub struct HttpDownloader {
    http_client: HttpClient,
}

impl HttpDownloader {
    /// Creates a new HttpDownloader with the given HTTP client.
    pub fn new(http_client: HttpClient) -> Self {
        Self { http_client }
    }

    /// Returns a reference to the underlying HTTP client.
    pub fn http_client(&self) -> &HttpClient {
        &self.http_client
    }
}

impl Downloader for HttpDownloader {
    fn download<'a, R: Runtime + 'a>(
        &'a self,
        runtime: &'a R,
        url: &'a str,
        dest: &'a Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { download_file(runtime, url, dest, &self.http_client).await })
    }
}

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
pub mod mock {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A mock downloader for testing that always succeeds.
    pub struct MockDownloader {
        should_fail: AtomicBool,
    }

    impl MockDownloader {
        pub fn new() -> Self {
            Self {
                should_fail: AtomicBool::new(false),
            }
        }

        pub fn set_should_fail(&self, fail: bool) {
            self.should_fail.store(fail, Ordering::SeqCst);
        }
    }

    impl Default for MockDownloader {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Downloader for MockDownloader {
        fn download<'a, R: Runtime + 'a>(
            &'a self,
            _runtime: &'a R,
            _url: &'a str,
            _dest: &'a Path,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
            let should_fail = self.should_fail.load(Ordering::SeqCst);
            Box::pin(async move {
                if should_fail {
                    anyhow::bail!("Mock download failed")
                } else {
                    Ok(())
                }
            })
        }
    }
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
