//! HTTP client module with retry logic and error handling.

mod client;
mod retry;

pub use client::HttpClient;
pub use retry::{check_retryable, classify_error, NonRetryableError, MAX_RETRIES, RETRY_DELAY_MS};
