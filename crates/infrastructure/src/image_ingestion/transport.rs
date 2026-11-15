//! Closed, versioned requests to the separately configured decoder.
use super::ImageError;
use commerce_image_codec::{Operation, Response, wire};
use std::{path::PathBuf, sync::OnceLock};

const DECODER_UID: u32 = 65532;
static DECODER: OnceLock<DecoderEndpoint> = OnceLock::new();

#[derive(Debug)]
pub struct DecoderEndpoint {
    socket: PathBuf,
    uid: u32,
}

impl DecoderEndpoint {
    pub fn parse(path: &str) -> Result<Self, &'static str> {
        let Some(relative) = path.strip_prefix('/') else {
            return Err("Invalid preview decoder socket");
        };
        if path.len() > 100
            || relative.split('/').any(|segment| {
                segment.is_empty()
                    || matches!(segment, "." | "..")
                    || !segment
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
            })
        {
            return Err("Invalid preview decoder socket");
        }
        Ok(Self {
            socket: path.into(),
            uid: DECODER_UID,
        })
    }

    #[cfg(unix)]
    pub(super) async fn exchange(
        &self,
        operation: Operation,
        bytes: &[u8],
    ) -> Result<Response, ImageError> {
        self.with_deadline(operation, bytes, std::time::Duration::from_secs(10))
            .await
    }

    #[cfg(unix)]
    async fn with_deadline(
        &self,
        operation: Operation,
        bytes: &[u8],
        deadline: std::time::Duration,
    ) -> Result<Response, ImageError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let header = wire::request_header(operation, bytes.len()).map_err(unavailable)?;
        tokio::time::timeout(deadline, async {
            let mut stream = tokio::net::UnixStream::connect(&self.socket)
                .await
                .map_err(unavailable)?;
            if stream.peer_cred().map_err(unavailable)?.uid() != self.uid {
                return Err(ImageError::ProcessingUnavailable);
            }
            stream.write_all(&header).await.map_err(unavailable)?;
            stream.write_all(bytes).await.map_err(unavailable)?;
            // Keep the write half open until the reply. Dropping this connection
            // lets the service cancel a running child when the caller disappears.
            let mut result = Vec::new();
            stream
                .take((wire::MAX_RESPONSE_BYTES + 1) as u64)
                .read_to_end(&mut result)
                .await
                .map_err(unavailable)?;
            wire::parse_response(&result).map_err(unavailable)
        })
        .await
        .map_err(unavailable)?
    }

    #[cfg(not(unix))]
    pub(super) async fn exchange(&self, _: Operation, _: &[u8]) -> Result<Response, ImageError> {
        Err(ImageError::ProcessingUnavailable)
    }
}

#[cfg(unix)]
fn unavailable<T>(_: T) -> ImageError {
    ImageError::ProcessingUnavailable
}

/// Install the decoder once, before the application starts accepting requests.
pub fn configure_decoder(endpoint: DecoderEndpoint) -> Result<(), &'static str> {
    DECODER
        .set(endpoint)
        .map_err(|_| "Preview decoder already configured")
}

pub(super) fn configured() -> Option<&'static DecoderEndpoint> {
    DECODER.get()
}

#[cfg(all(test, unix))]
#[path = "../../tests/support/image_decoder_transport.rs"]
mod tests;
