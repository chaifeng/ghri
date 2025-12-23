//! Retry logic for network operations with intelligent error classification.

use reqwest::StatusCode;

/// Maximum number of retry attempts for network operations.
pub const MAX_RETRIES: usize = 3;

/// Delay between retry attempts in milliseconds.
pub const RETRY_DELAY_MS: u64 = 1000;

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
                write!(
                    f,
                    "Rate limit exceeded: {}. Try again later or set GITHUB_TOKEN environment variable.",
                    msg
                )
            }
            NonRetryableError::AuthenticationFailed(msg) => {
                write!(
                    f,
                    "Authentication failed: {}. Check your GITHUB_TOKEN.",
                    msg
                )
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_non_retryable_error_client_error_display() {
        let err = NonRetryableError::ClientError("HTTP 400".to_string());
        assert!(err.to_string().contains("Request error"));
        assert!(err.to_string().contains("HTTP 400"));
    }

    #[tokio::test]
    async fn test_classify_error_unauthorized() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(401)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = classify_error(&err);
        assert!(matches!(
            result,
            Err(NonRetryableError::AuthenticationFailed(_))
        ));
    }

    #[tokio::test]
    async fn test_classify_error_forbidden() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(403)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = classify_error(&err);
        assert!(matches!(result, Err(NonRetryableError::Forbidden(_))));
    }

    #[tokio::test]
    async fn test_classify_error_too_many_requests() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(429)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = classify_error(&err);
        assert!(matches!(
            result,
            Err(NonRetryableError::RateLimitExceeded(_))
        ));
    }

    #[tokio::test]
    async fn test_classify_error_not_found() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(404)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = classify_error(&err);
        assert!(matches!(result, Err(NonRetryableError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_classify_error_other_client_error() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(400)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = classify_error(&err);
        assert!(matches!(result, Err(NonRetryableError::ClientError(_))));
    }

    #[tokio::test]
    async fn test_classify_error_server_error_is_retryable() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(500)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = classify_error(&err);
        assert!(result.is_ok()); // Server errors are retryable
    }

    #[tokio::test]
    async fn test_check_retryable_non_retryable() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(404)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = check_retryable(err);
        assert!(result.downcast_ref::<NonRetryableError>().is_some());
    }

    #[tokio::test]
    async fn test_check_retryable_retryable() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(503)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let response = client.get(server.url()).send().await.unwrap();
        let err = response.error_for_status().unwrap_err();

        let result = check_retryable(err);
        // Server errors are retryable, so it should remain as reqwest::Error
        assert!(result.downcast_ref::<NonRetryableError>().is_none());
    }
}
