//! LSP base-protocol framing: `Content-Length` headers around JSON bodies.

use std::io;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};

/// Largest message body accepted from a server.
///
/// A hostile or broken server must not be able to allocate without bound just
/// by announcing a huge `Content-Length`.
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    Malformed(String),
    TooLarge(usize),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "language server I/O failed: {error}"),
            Self::Malformed(reason) => {
                write!(formatter, "malformed language server message: {reason}")
            }
            Self::TooLarge(bytes) => write!(
                formatter,
                "language server message of {bytes} bytes exceeds the {MAX_MESSAGE_BYTES} byte limit"
            ),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Encode one JSON-RPC message with its `Content-Length` header.
pub fn encode(message: &Value) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(message).map_err(|error| {
        FrameError::Malformed(format!("could not serialize a message: {error}"))
    })?;
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(FrameError::TooLarge(body.len()));
    }
    let mut framed = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    framed.extend_from_slice(&body);
    Ok(framed)
}

/// Reads `Content-Length`-framed JSON messages from an async stream.
pub struct FrameReader<R> {
    reader: R,
    buffer: Vec<u8>,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: Vec::new(),
        }
    }

    /// Read the next message; `None` is a clean end of stream.
    pub async fn read_message(&mut self) -> Result<Option<Value>, FrameError> {
        loop {
            if let Some(message) = self.take_frame()? {
                return Ok(Some(message));
            }
            let mut chunk = [0u8; 8 * 1024];
            let read = self.reader.read(&mut chunk).await?;
            if read == 0 {
                return if self.buffer.is_empty() {
                    Ok(None)
                } else {
                    Err(FrameError::Malformed(
                        "stream ended inside a message".into(),
                    ))
                };
            }
            self.buffer.extend_from_slice(&chunk[..read]);
            if self.buffer.len() > MAX_MESSAGE_BYTES + 8 * 1024 {
                return Err(FrameError::TooLarge(self.buffer.len()));
            }
        }
    }

    /// Pop one complete frame from the buffer, if the buffer holds one.
    fn take_frame(&mut self) -> Result<Option<Value>, FrameError> {
        let Some((header_end, header_len)) = header_boundary(&self.buffer) else {
            return Ok(None);
        };
        let headers = std::str::from_utf8(&self.buffer[..header_end])
            .map_err(|_| FrameError::Malformed("headers are not valid UTF-8".into()))?;
        let Some(length) = content_length(headers)? else {
            return Err(FrameError::Malformed(
                "missing Content-Length header".into(),
            ));
        };
        if length > MAX_MESSAGE_BYTES {
            return Err(FrameError::TooLarge(length));
        }
        let body_start = header_end + header_len;
        let body_end = body_start + length;
        if self.buffer.len() < body_end {
            return Ok(None);
        }
        let body = self.buffer[body_start..body_end].to_vec();
        self.buffer.drain(..body_end);
        let message = serde_json::from_slice(&body)
            .map_err(|error| FrameError::Malformed(format!("invalid JSON body: {error}")))?;
        Ok(Some(message))
    }
}

/// Find the header terminator and its length, accepting CRLF and bare LF.
fn header_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4))
        .or_else(|| {
            buffer
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|index| (index, 2))
        })
}

fn content_length(headers: &str) -> Result<Option<usize>, FrameError> {
    for line in headers.split(['\r', '\n']) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value
                .trim()
                .parse::<usize>()
                .map(Some)
                .map_err(|_| FrameError::Malformed(format!("invalid Content-Length {value:?}")));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn encodes_a_content_length_header() {
        let frame = encode(&json!({"jsonrpc": "2.0", "id": 1})).unwrap();
        let split = frame
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        let header = std::str::from_utf8(&frame[..split]).unwrap();
        let body = &frame[split + 4..];
        assert_eq!(header, format!("Content-Length: {}", body.len()));
        let body = serde_json::from_slice::<Value>(body).unwrap();
        assert_eq!(body["id"], 1);
    }

    #[tokio::test]
    async fn reads_frames_split_across_reads() {
        let (mut writer, reader) = tokio::io::duplex(8);
        let first = json!({"jsonrpc": "2.0", "method": "window/logMessage"});
        let second = json!({"jsonrpc": "2.0", "id": 2, "result": null});
        let mut bytes = encode(&first).unwrap();
        bytes.extend(encode(&second).unwrap());
        let task = tokio::spawn(async move {
            for chunk in bytes.chunks(5) {
                writer.write_all(chunk).await.unwrap();
            }
            writer.shutdown().await.unwrap();
        });

        let mut frames = FrameReader::new(reader);
        assert_eq!(frames.read_message().await.unwrap(), Some(first));
        assert_eq!(frames.read_message().await.unwrap(), Some(second));
        assert_eq!(frames.read_message().await.unwrap(), None);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_a_body_larger_than_announced() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let task = tokio::spawn(async move {
            writer
                .write_all(b"Content-Length: 4\r\n\r\n{")
                .await
                .unwrap();
            writer.shutdown().await.unwrap();
        });
        let error = FrameReader::new(reader).read_message().await.unwrap_err();
        assert!(matches!(error, FrameError::Malformed(_)), "{error}");
        task.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_a_missing_content_length() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let task = tokio::spawn(async move {
            writer.write_all(b"X-Nope: 1\r\n\r\n{}").await.unwrap();
            writer.shutdown().await.unwrap();
        });
        let error = FrameReader::new(reader).read_message().await.unwrap_err();
        assert!(
            matches!(&error, FrameError::Malformed(reason) if reason.contains("Content-Length")),
            "{error}"
        );
        task.await.unwrap();
    }

    #[test]
    fn refuses_oversized_bodies() {
        let huge = serde_json::Value::String("x".repeat(MAX_MESSAGE_BYTES));
        assert!(matches!(encode(&huge), Err(FrameError::TooLarge(_))));
    }
}
