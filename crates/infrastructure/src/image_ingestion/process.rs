//! Preserve input and custody checks around the codec, outside DB transactions.
use super::{ImageError, transport};
use commerce_domain::media::{ImageType, MediaAssetDraft};
use commerce_image_codec::{Kind, MAX_RENDITION_BYTES, Operation, Prepared, Rejection, Response};
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, SemaphorePermit};

static DECODERS: Semaphore = Semaphore::const_new(2);

#[derive(Clone, Debug)]
pub(super) struct Rendition {
    pub edge: i32,
    pub width: i32,
    pub height: i32,
    pub image_type: &'static str,
    pub sha256: [u8; 32],
    pub bytes: Vec<u8>,
}

pub(super) struct Decoded {
    pub registered: MediaAssetDraft,
    pub original: Vec<u8>,
    pub sha256: [u8; 32],
    pub width: i32,
    pub height: i32,
    pub renditions: Vec<Rendition>,
}

impl From<Rejection> for ImageError {
    fn from(value: Rejection) -> Self {
        match value {
            Rejection::MetadataMismatch => Self::MetadataMismatch,
            Rejection::InvalidImage => Self::InvalidImage,
            Rejection::UnsupportedImage => Self::UnsupportedImage,
            Rejection::ResourceLimit => Self::ResourceLimit,
            Rejection::InvalidStoredData => Self::InvalidStoredData,
            Rejection::Unavailable => Self::ProcessingUnavailable,
        }
    }
}

async fn run_codec(
    endpoint: Option<&transport::DecoderEndpoint>,
    operation: Operation,
    bytes: Vec<u8>,
    permit: SemaphorePermit<'static>,
) -> Result<(Response, Vec<u8>), ImageError> {
    if let Some(endpoint) = endpoint {
        let _permit = permit;
        let response = endpoint.exchange(operation, &bytes).await?;
        return Ok((response, bytes));
    }
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        (commerce_image_codec::process(operation, &bytes), bytes)
    })
    .await
    .map_err(|_| ImageError::ProcessingUnavailable)
}

fn assemble(
    registered: MediaAssetDraft,
    original: Vec<u8>,
    sha256: [u8; 32],
    prepared: Prepared,
) -> Result<Decoded, ImageError> {
    let dims = (i64::from(prepared.width), i64::from(prepared.height));
    let raw = (
        i64::from(registered.width_px),
        i64::from(registered.height_px),
    );
    if dims != raw && dims != (raw.1, raw.0) {
        return Err(ImageError::InvalidStoredData);
    }
    let renditions = prepared
        .renditions
        .into_iter()
        .map(|rendition| {
            let image_type = match rendition.kind {
                Kind::Jpeg => "jpeg",
                Kind::Png => "png",
                Kind::Webp => return Err(ImageError::InvalidStoredData),
            };
            Ok(Rendition {
                edge: rendition.edge,
                width: rendition.width,
                height: rendition.height,
                image_type,
                sha256: Sha256::digest(&rendition.bytes).into(),
                bytes: rendition.bytes,
            })
        })
        .collect::<Result<_, ImageError>>()?;
    Ok(Decoded {
        registered,
        original,
        sha256,
        width: prepared.width,
        height: prepared.height,
        renditions,
    })
}

pub(super) async fn run(metadata: MediaAssetDraft, bytes: Vec<u8>) -> Result<Decoded, ImageError> {
    let permit = DECODERS.try_acquire().map_err(|_| ImageError::Busy)?;
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    if digest != metadata.sha256 {
        return Err(ImageError::MetadataMismatch);
    }
    let kind = match metadata.image_type {
        ImageType::Jpeg => Kind::Jpeg,
        ImageType::Png => Kind::Png,
        ImageType::Webp => Kind::Webp,
    };
    let (reply, original) = run_codec(
        transport::configured(),
        Operation::Prepare {
            kind,
            width: metadata.width_px,
            height: metadata.height_px,
        },
        bytes,
        permit,
    )
    .await?;
    match reply {
        Response::Prepared(output) => assemble(metadata, original, digest, output),
        Response::Rejected(error) => Err(error.into()),
        Response::Valid => Err(ImageError::ProcessingUnavailable),
    }
}

pub(super) async fn validate_output(
    kind: String,
    width: i32,
    height: i32,
    bytes: Vec<u8>,
    digest: Vec<u8>,
) -> Result<super::PrivateRendition, ImageError> {
    let permit = DECODERS.try_acquire().map_err(|_| ImageError::Busy)?;
    if bytes.is_empty()
        || bytes.len() > MAX_RENDITION_BYTES
        || !(1..=960).contains(&width)
        || !(1..=960).contains(&height)
        || Sha256::digest(&bytes)[..] != digest
    {
        return Err(ImageError::InvalidStoredData);
    }
    let format = match kind.as_str() {
        "jpeg" => Kind::Jpeg,
        "png" => Kind::Png,
        _ => return Err(ImageError::InvalidStoredData),
    };
    let (reply, bytes) = run_codec(
        transport::configured(),
        Operation::Validate {
            kind: format,
            width: u32::try_from(width).map_err(|_| ImageError::InvalidStoredData)?,
            height: u32::try_from(height).map_err(|_| ImageError::InvalidStoredData)?,
        },
        bytes,
        permit,
    )
    .await?;
    match reply {
        Response::Valid => Ok(super::PrivateRendition {
            image_type: kind,
            width,
            height,
            bytes,
        }),
        Response::Rejected(Rejection::Unavailable) => Err(ImageError::ProcessingUnavailable),
        _ => Err(ImageError::InvalidStoredData),
    }
}

#[cfg(test)]
#[path = "../../tests/support/image_processor.rs"]
mod tests;
