//! Retry logic for network operations with intelligent error classification.

use anyhow::{anyhow, Result};
use log::{debug, warn};
use reqwest::StatusCode;

/// Maximum number of retry attempts for network operations.
pub const MAX_RETRIES: usize = 3;

/// Delay between retry attempts in milliseconds.
const RETRY_DELAY_MS: u64 = 1000;

/// Errors that should not be retried.
#[derive(Debug)]
pub enum NonRetryableError {
    /// Rate limit exceeded (HTTP 403 with rate limit message or 429)
    RateLimitExceeded(String),
    /// Authentication failed (HTTP 401)
    AuthenticationFailed(String),
    /// Resource not found (HTTP 404)
    NotFound(String),
    /// Forbidden access (HTTP 403 non-rate-limit)
    Forbidden(String),
    /// Other client errors that won't succeed on retry
    ClientError(String),
}

impl std::fmt::Display for NonRetryableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NonRetryableError::RateLimitExceeded(msg) => {
                write!(f, "Rate limit exceeded: {}. Try again later or set GITHUB_TOKEN environment variable.", msg)
            }
            NonRetryableError::AuthenticationFailed(msg) => {
                write!(f, "Authentication failed: {}. Check your GITHUB_TOKEN.", msg)
            }
            NonRetryableError::NotFound(msg) => {
                write!(f, "Not found: {}", msg)
            }
            NonRetryableError::Forbidden(msg) => {
                write!(f, "Access forbidden: {}. You may need authentication.", msg)
            }
            NonRetryableError::ClientError(msg) => {
                write!(f, "Request error: {}", msg)
            }
        }
    }
}

impl std::error::Error for NonRetryableError {}

/// Classifies an error as retryable or non-retryable.
/// Returns Ok(()) if the error is retryable, Err with a user-friendly message if not.
pub fn classify_error(error: &reqwest::Error) -> Result<(), NonRetryableError> {
    if let Some(status) = error.status() {
        match status {
            StatusCode::UNAUTHORIZED => {
                return Err(NonRetryableError::AuthenticationFailed(
                    "Invalid or missing authentication token".to_string(),
                ));
            }
            StatusCode::FORBIDDEN => {
                let msg = error.to_string();
                if msg.contains("rate limit") || msg.contains("API rate limit") {
                    return Err(NonRetryableError::RateLimitExceeded(
                        "GitHub API rate limit exceeded".to_string(),
                    ));
                }
                return Err(NonRetryableError::Forbidden(
                    "Access to this resource is forbidden".to_string(),
                ));
            }
            StatusCode::TOO_MANY_REQUESTS => {
                return Err(NonRetryableError::RateLimitExceeded(
                    "Too many requests".to_string(),
                ));
            }
            StatusCode::NOT_FOUND => {
                return Err(NonRetryableError::NotFound(
                    "The requested resource was not found".to_string(),
                ));
            }
            // Other 4xx client errors are generally not retryable
            s if s.is_client_error() => {
                return Err(NonRetryableError::ClientError(format!(
                    "HTTP {} error",
                    s.as_u16()
                )));
            }
            // 5xx server errors are retryable
            _ => {}
        }
    }

    // Connection errors, timeouts, etc. are retryable
    Ok(())
}

/// Checks if an error from `error_for_status()` should be retried.
/// Returns the original error if retryable, or a user-friendly NonRetryableError if not.
pub fn check_retryable(error: reqwest::Error) -> anyhow::Error {
    match classify_error(&error) {
        Ok(()) => anyhow::Error::from(error),
        Err(non_retryable) => anyhow::Error::from(non_retryable),
    }
}

/// Checks if an anyhow::Error is retryable based on its content.
fn is_retryable_error(e: &anyhow::Error) -> bool {
    // Non-retryable errors should not be retried
    if e.downcast_ref::<NonRetryableError>().is_some() {
        return false;
    }

    // Check if this looks like a network error we can retry
    let error_str = e.to_string();
    error_str.contains("connection")
        || error_str.contains("timeout")
        || error_str.contains("reset")
        || error_str.contains("broken pipe")
        || error_str.contains("dns")
        || error_str.contains("resolve")
}

/// Executes an async operation with retry logic.
/// Only retries on network errors and server errors (5xx).
/// Immediately fails on client errors (4xx) with user-friendly messages.
pub async fn with_retry<F, Fut, T>(operation_name: &str, operation: F) -> Result<T>
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

    Err(last_error.unwrap_or_else(|| anyhow!("{}: failed after {} attempts", operation_name, MAX_RETRIES)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_non_retryable_error_display() {
        let err = NonRetryableError::RateLimitExceeded("test".to_string());
        assert!(err.to_string().contains("Rate limit"));
        assert!(err.to_string().contains("GITHUB_TOKEN"));

        let err = NonRetryableError::AuthenticationFailed("test".to_string());
        assert!(err.to_string().contains("Authentication"));

        let err = NonRetryableError::NotFound("test".to_string());
        assert!(err.to_string().contains("Not found"));

        let err = NonRetryableError::Forbidden("test".to_string());
        assert!(err.to_string().contains("forbidden"));
    }

    #[tokio::test]
    async fn test_with_retry_success() {
        let result = with_retry("test", || async { Ok::<_, anyhow::Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_with_retry_immediate_failure_on_non_retryable() {
        let start = std::time::Instant::now();
        let result = with_retry("test", || async {
            Err::<i32, _>(anyhow::Error::from(NonRetryableError::NotFound(
                "test".to_string(),
            )))
        })
        .await;

        // Should fail immediately without retries (well under 1 second)
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn test_is_retryable_error() {
        // Non-retryable error
        let err = anyhow::Error::from(NonRetryableError::NotFound("test".to_string()));
        assert!(!is_retryable_error(&err));

        // Network-like error (retryable)
        let err = anyhow::anyhow!("connection reset by peer");
        assert!(is_retryable_error(&err));

        // Generic error (not retryable)
        let err = anyhow::anyhow!("some other error");
        assert!(!is_retryable_error(&err));
    }

    #[test]
    fn test_non_retryable_error_client_error_display() {
        let err = NonRetryableError::ClientError("HTTP 400".to_string());
        assert!(err.to_string().contains("Request error"));
        assert!(err.to_string().contains("HTTP 400"));
    }

    #[test]
    fn test_is_retryable_error_timeout() {
        let err = anyhow::anyhow!("operation timeout occurred");
        assert!(is_retryable_error(&err));
    }

    #[test]
    fn test_is_retryable_error_broken_pipe() {
        let err = anyhow::anyhow!("broken pipe error");
        assert!(is_retryable_error(&err));
    }

    #[test]
    fn test_is_retryable_error_dns() {
        let err = anyhow::anyhow!("dns resolution failed");
        assert!(is_retryable_error(&err));
    }

    #[test]
    fn test_is_retryable_error_resolve() {
        let err = anyhow::anyhow!("could not resolve host");
        assert!(is_retryable_error(&err));
    }

    #[tokio::test]
    async fn test_with_retry_retries_on_network_error() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result = with_retry("test", || {
            let attempts = Arc::clone(&attempts_clone);
            async move {
                let count = attempts.fetch_add(1, Ordering::SeqCst);
                if count < 2 {
                    Err::<i32, _>(anyhow::anyhow!("connection reset"))
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_with_retry_exhausts_retries() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result = with_retry("test", || {
            let attempts = Arc::clone(&attempts_clone);
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(anyhow::anyhow!("connection timeout"))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), MAX_RETRIES);
    }

    #[tokio::test]
    async fn test_classify_error_unauthorized() {
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/").with_status(401).create_async().await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = classify_error(&err);
        assert!(matches!(result, Err(NonRetryableError::AuthenticationFailed(_))));
    }

    #[tokio::test]
    async fn test_classify_error_forbidden() {
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/").with_status(403).create_async().await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = classify_error(&err);
        assert!(matches!(result, Err(NonRetryableError::Forbidden(_))));
    }

    #[tokio::test]
    async fn test_classify_error_too_many_requests() {
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/").with_status(429).create_async().await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = classify_error(&err);
        assert!(matches!(result, Err(NonRetryableError::RateLimitExceeded(_))));
    }

    #[tokio::test]
    async fn test_classify_error_not_found() {
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/").with_status(404).create_async().await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = classify_error(&err);
        assert!(matches!(result, Err(NonRetryableError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_classify_error_other_client_error() {
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/").with_status(400).create_async().await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = classify_error(&err);
        assert!(matches!(result, Err(NonRetryableError::ClientError(_))));
    }

    #[tokio::test]
    async fn test_classify_error_server_error_is_retryable() {
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/").with_status(500).create_async().await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = classify_error(&err);
        assert!(result.is_ok()); // Server errors are retryable
    }

    #[tokio::test]
    async fn test_check_retryable_non_retryable() {
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/").with_status(404).create_async().await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = check_retryable(err);
        assert!(result.downcast_ref::<NonRetryableError>().is_some());
    }

    #[tokio::test]
    async fn test_check_retryable_retryable() {
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/").with_status(503).create_async().await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = check_retryable(err);
        // Server errors are retryable, so it should remain as reqwest::Error
        assert!(result.downcast_ref::<NonRetryableError>().is_none());
    }
}
