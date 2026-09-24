//! Small native transport primitives shared by asset consumers.
//!
//! This crate deliberately stops at HTTP request policy and byte transfer. It
//! does not know about manifests, archives, image formats, or dataset state, so
//! a UI update check and the networking byte plane do not inherit the offline
//! asset baker's dependency graph.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use lunco_settings::DownloadSettings;
    use std::fmt;
    use std::io::{Cursor, Read, Seek, SeekFrom, Write};

    /// Maximum interval ureq waits for the next body bytes.
    pub const BODY_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
    /// Timeout for establishing a TCP/TLS connection.
    pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
    /// Timeout for sending request headers on an established connection.
    pub const SEND_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
    /// Timeout for receiving response headers.
    pub const RECV_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

    /// Returns whether a failed request is worth trying again.
    pub fn is_retryable_download_error(error: &ureq::Error) -> bool {
        match error {
            ureq::Error::StatusCode(code) => matches!(code, 408 | 425 | 429 | 500..=599),
            ureq::Error::Io(_)
            | ureq::Error::Timeout(_)
            | ureq::Error::HostNotFound
            | ureq::Error::ConnectionFailed
            | ureq::Error::Protocol(_) => true,
            _ => false,
        }
    }

    /// Run one retryable operation under the application-wide download policy.
    ///
    /// `max_attempts` is the total number of requests, including the first
    /// one. `should_continue` is checked while waiting so an owned task can
    /// cancel promptly.
    pub fn retry_with_backoff<T, E, Operation, Retryable, Continue>(
        settings: &DownloadSettings,
        mut operation: Operation,
        mut retryable: Retryable,
        mut should_continue: Continue,
    ) -> Result<T, E>
    where
        Operation: FnMut() -> Result<T, E>,
        Retryable: FnMut(&E) -> bool,
        Continue: FnMut() -> bool,
    {
        let attempts = settings.max_attempts.max(1);
        for attempt in 1..=attempts {
            match operation() {
                Ok(value) => return Ok(value),
                Err(error) if attempt < attempts && retryable(&error) => {
                    let deadline = std::time::Instant::now() + settings.retry_delay(attempt);
                    while std::time::Instant::now() < deadline {
                        if !should_continue() {
                            return Err(error);
                        }
                        let remaining =
                            deadline.saturating_duration_since(std::time::Instant::now());
                        std::thread::sleep(remaining.min(std::time::Duration::from_millis(100)));
                    }
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("download retry policy always performs at least one attempt")
    }

    /// Failure reported by the shared resumable byte-transfer primitive.
    #[derive(Debug)]
    pub enum TransferError {
        /// The caller stopped the transfer.
        Cancelled,
        /// The HTTP request failed.
        Request(String),
        /// Reading the response body failed or ended before its advertised size.
        Body(String),
        /// The caller's output sink could not be reset or written.
        Write(String),
        /// The server returned a response that violates the resume contract.
        Protocol(String),
    }

    impl fmt::Display for TransferError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Cancelled => formatter.write_str("transfer cancelled"),
                Self::Request(error) => write!(formatter, "request failed: {error}"),
                Self::Body(error) => write!(formatter, "response body failed: {error}"),
                Self::Write(error) => write!(formatter, "output write failed: {error}"),
                Self::Protocol(error) => write!(formatter, "invalid transfer response: {error}"),
            }
        }
    }

    impl std::error::Error for TransferError {}

    /// Final byte counts for one successful transfer.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct TransferStats {
        /// Number of bytes written to the sink.
        pub bytes_done: u64,
        /// Advertised total, or zero when the server did not provide one.
        pub bytes_total: u64,
    }

    enum ResumableTransferError {
        Request(ureq::Error),
        Body(ureq::Error),
        Write(String),
        Protocol(String),
        Cancelled,
    }

    fn resumable_transfer_error_is_retryable(error: &ResumableTransferError) -> bool {
        match error {
            ResumableTransferError::Request(error) | ResumableTransferError::Body(error) => {
                is_retryable_download_error(error)
            }
            ResumableTransferError::Write(_)
            | ResumableTransferError::Protocol(_)
            | ResumableTransferError::Cancelled => false,
        }
    }

    fn content_range_start_and_total(
        response: &ureq::http::Response<ureq::Body>,
    ) -> Option<(u64, Option<u64>)> {
        let value = response.headers().get("content-range")?.to_str().ok()?;
        let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
        let (start, _) = range.split_once('-')?;
        Some((start.parse().ok()?, total.parse().ok()))
    }

    /// Stream a response into a seekable output while retaining a received
    /// prefix across retry attempts. A range-capable server resumes at the
    /// prefix; a server that ignores `Range` invokes `reset` and safely starts
    /// a fresh response. This is the only owner of native HTTP resume policy.
    pub fn download_to_writer<Writer, Reset, Progress, Continue>(
        url: &str,
        settings: &DownloadSettings,
        writer: &mut Writer,
        mut reset: Reset,
        mut progress: Progress,
        mut should_continue: Continue,
    ) -> Result<TransferStats, TransferError>
    where
        Writer: Write + Seek,
        Reset: FnMut(&mut Writer) -> Result<(), String>,
        Progress: FnMut(&[u8], u64, u64),
        Continue: FnMut() -> bool,
    {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_send_request(Some(SEND_REQUEST_TIMEOUT))
            .timeout_recv_response(Some(RECV_RESPONSE_TIMEOUT))
            .timeout_recv_body(Some(BODY_READ_TIMEOUT))
            .build()
            .into();
        let mut downloaded = writer
            .seek(SeekFrom::End(0))
            .map_err(|error| TransferError::Write(error.to_string()))?;
        let mut total = 0_u64;
        let mut chunk = [0_u8; 64 * 1024];
        let continuation = std::cell::RefCell::new(&mut should_continue);
        let result = retry_with_backoff(
            settings,
            || {
                if !(continuation.borrow_mut())() {
                    return Err(ResumableTransferError::Cancelled);
                }
                let mut request = agent.get(url);
                if downloaded > 0 {
                    request = request.header("Range", &format!("bytes={downloaded}-"));
                }
                let response = request.call().map_err(ResumableTransferError::Request)?;
                let status = response.status().as_u16();
                if status != 200 && status != 206 {
                    return Err(ResumableTransferError::Protocol(format!(
                        "HTTP {status} cannot complete byte fetch from offset {downloaded}"
                    )));
                }

                if status == 206 {
                    let Some((start, response_total)) = content_range_start_and_total(&response)
                    else {
                        return Err(ResumableTransferError::Protocol(
                            "206 response omitted a valid Content-Range".into(),
                        ));
                    };
                    if start != downloaded {
                        return Err(ResumableTransferError::Protocol(format!(
                            "server resumed at byte {start}, requested {downloaded}"
                        )));
                    }
                    total = response_total.unwrap_or_else(|| {
                        response
                            .headers()
                            .get("content-length")
                            .and_then(|value| value.to_str().ok())
                            .and_then(|value| value.parse::<u64>().ok())
                            .map(|length| downloaded.saturating_add(length))
                            .unwrap_or(0)
                    });
                    writer
                        .seek(SeekFrom::End(0))
                        .map_err(|error| ResumableTransferError::Write(error.to_string()))?;
                } else {
                    reset(writer).map_err(ResumableTransferError::Write)?;
                    downloaded = 0;
                    total = response
                        .headers()
                        .get("content-length")
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(0);
                }

                let mut reader = response.into_body().into_reader();
                loop {
                    if !(continuation.borrow_mut())() {
                        return Err(ResumableTransferError::Cancelled);
                    }
                    let count = reader
                        .read(&mut chunk)
                        .map_err(|error| ResumableTransferError::Body(ureq::Error::Io(error)))?;
                    if count == 0 {
                        break;
                    }
                    writer
                        .write_all(&chunk[..count])
                        .map_err(|error| ResumableTransferError::Write(error.to_string()))?;
                    downloaded = downloaded.saturating_add(count as u64);
                    progress(&chunk[..count], downloaded, total);
                }
                if total != 0 && downloaded < total {
                    return Err(ResumableTransferError::Body(ureq::Error::Io(
                        std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            format!("received {downloaded} of {total} bytes"),
                        ),
                    )));
                }
                Ok(())
            },
            resumable_transfer_error_is_retryable,
            || (continuation.borrow_mut())(),
        );
        result
            .map(|()| TransferStats {
                bytes_done: downloaded,
                bytes_total: total,
            })
            .map_err(|error| match error {
                ResumableTransferError::Request(error) => TransferError::Request(error.to_string()),
                ResumableTransferError::Body(error) => TransferError::Body(error.to_string()),
                ResumableTransferError::Write(error) => TransferError::Write(error),
                ResumableTransferError::Protocol(error) => TransferError::Protocol(error),
                ResumableTransferError::Cancelled => TransferError::Cancelled,
            })
    }

    /// Fetch a complete response while retaining a received prefix across
    /// retry attempts. This convenience wrapper uses the shared writer
    /// transfer with an in-memory sink.
    pub fn download_bytes_with_resume(
        url: &str,
        settings: &DownloadSettings,
    ) -> Result<Vec<u8>, String> {
        let mut sink = Cursor::new(Vec::new());
        download_to_writer(
            url,
            settings,
            &mut sink,
            |sink| {
                sink.get_mut().clear();
                sink.set_position(0);
                Ok(())
            },
            |_chunk, _done, _total| {},
            || true,
        )
        .map(|_| sink.into_inner())
        .map_err(|error| error.to_string())
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{
    BODY_READ_TIMEOUT, CONNECT_TIMEOUT, RECV_RESPONSE_TIMEOUT, SEND_REQUEST_TIMEOUT, TransferError,
    TransferStats, download_bytes_with_resume, download_to_writer, is_retryable_download_error,
    retry_with_backoff,
};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use lunco_settings::DownloadSettings;

    #[test]
    fn retry_classification_keeps_not_found_terminal() {
        assert!(is_retryable_download_error(&ureq::Error::ConnectionFailed));
        assert!(is_retryable_download_error(&ureq::Error::StatusCode(503)));
        assert!(!is_retryable_download_error(&ureq::Error::StatusCode(404)));
    }

    #[test]
    fn shared_retry_policy_retries_transient_operations_only_to_success() {
        let settings = DownloadSettings {
            max_attempts: 3,
            retry_initial_delay_secs: 0,
            ..Default::default()
        };
        let mut calls = 0;
        let value = retry_with_backoff(
            &settings,
            || {
                calls += 1;
                if calls < 3 { Err("transient") } else { Ok(42) }
            },
            |error| *error == "transient",
            || true,
        )
        .expect("the final configured attempt succeeds");
        assert_eq!(value, 42);
        assert_eq!(calls, 3);
    }

    #[test]
    fn shared_retry_policy_stops_on_non_retryable_or_cancelled_errors() {
        let settings = DownloadSettings {
            max_attempts: 5,
            retry_initial_delay_secs: 1,
            ..Default::default()
        };
        let mut non_retryable_calls = 0;
        let error = retry_with_backoff(
            &settings,
            || {
                non_retryable_calls += 1;
                Err::<(), _>("permanent")
            },
            |_| false,
            || true,
        )
        .expect_err("a permanent error is not retried");
        assert_eq!(error, "permanent");
        assert_eq!(non_retryable_calls, 1);

        let mut cancelled_calls = 0;
        let error = retry_with_backoff(
            &settings,
            || {
                cancelled_calls += 1;
                Err::<(), _>("transient")
            },
            |_| true,
            || false,
        )
        .expect_err("cancellation returns the current operation error");
        assert_eq!(error, "transient");
        assert_eq!(cancelled_calls, 1);
    }

    #[test]
    fn byte_download_resumes_a_truncated_response() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read test server address");
        let server = std::thread::spawn(move || {
            for (index, expected_range) in [None, Some("bytes=3-")].into_iter().enumerate() {
                let (mut stream, _) = listener.accept().expect("accept byte request");
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 256];
                    let length = stream.read(&mut chunk).expect("read byte request");
                    request.extend_from_slice(&chunk[..length]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
                match expected_range {
                    Some(range) => assert!(request.contains(&format!("range: {range}"))),
                    None => assert!(!request.contains("range:")),
                }
                if index == 0 {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc")
                        .expect("write truncated response");
                } else {
                    stream
                        .write_all(
                            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 7\r\nContent-Range: bytes 3-9/10\r\n\r\ndefghij",
                        )
                        .expect("write resumed response");
                }
            }
        });

        let settings = DownloadSettings {
            max_attempts: 2,
            retry_initial_delay_secs: 0,
            ..Default::default()
        };
        let bytes = download_bytes_with_resume(&format!("http://{address}"), &settings)
            .expect("truncated body resumes from the received prefix");
        server.join().expect("resume server completed");
        assert_eq!(bytes, b"abcdefghij");
    }
}
