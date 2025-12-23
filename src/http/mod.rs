//! HTTP client module with retry logic and error handling.

mod client;
mod retry;

pub use client::HttpClient;
pub use retry::{MAX_RETRIES, NonRetryableError, RETRY_DELAY_MS, check_retryable, classify_error};
