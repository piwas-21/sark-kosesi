//! CPU-only validation and metadata-free derivatives, outside any database transaction.
use super::ImageError;
use commerce_domain::media::{ImageType, MediaAssetDraft};
use image::{
    DynamicImage, ImageDecoder, ImageFormat, Limits,
    codecs::{
        jpeg::{JpegDecoder, JpegEncoder},
        png::PngDecoder,
        webp::WebPDecoder,
    },
};
use sha2::{Digest, Sha256};
use std::io::Cursor;
use tokio::sync::Semaphore;
static DECODERS: Semaphore = Semaphore::const_new(2);
pub(super) const MAX_RENDITION_BYTES: usize = 250 * 1024;
const MAX_DECODED_BYTES: u64 = 128 * 1024 * 1024;
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
fn invalid_stored_image(_: image::ImageError) -> ImageError {
    ImageError::InvalidStoredData
}
fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    limits
}
fn decode<'a>(
    bytes: &'a [u8],
    format: ImageFormat,
) -> Result<Box<dyn ImageDecoder + 'a>, ImageError> {
    let mut decoder: Box<dyn ImageDecoder> = match format {
        ImageFormat::Jpeg => Box::new(JpegDecoder::new(Cursor::new(bytes))?),
        ImageFormat::Png => {
            let decoder = PngDecoder::with_limits(Cursor::new(bytes), limits())?;
            if decoder.is_apng()? {
                return Err(ImageError::UnsupportedImage);
            }
            Box::new(decoder)
        }
        ImageFormat::WebP => {
            super::framing::webp(bytes)?;
            let decoder = WebPDecoder::new(Cursor::new(bytes))?;
            if decoder.has_animation() {
                return Err(ImageError::UnsupportedImage);
            }
            Box::new(decoder)
        }
        _ => return Err(ImageError::UnsupportedImage),
    };
    decoder
        .set_limits(limits())
        .map_err(|_| ImageError::ResourceLimit)?;
    Ok(decoder)
}
fn prepare(metadata: &MediaAssetDraft, bytes: Vec<u8>) -> Result<Decoded, ImageError> {
    // The sole ingestion entry point bounds nonempty bytes before copying/spawning.
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    if digest != metadata.sha256 {
        return Err(ImageError::MetadataMismatch);
    }
    let format = image::guess_format(&bytes).map_err(|_| ImageError::UnsupportedImage)?;
    let expected = match metadata.image_type {
        ImageType::Jpeg => ImageFormat::Jpeg,
        ImageType::Png => ImageFormat::Png,
        ImageType::Webp => ImageFormat::WebP,
    };
    if format != expected {
        return Err(ImageError::MetadataMismatch);
    }
    let mut decoder = decode(&bytes, format)?;
    if decoder.dimensions() != (metadata.width_px, metadata.height_px) {
        return Err(ImageError::MetadataMismatch);
    }
    if decoder.total_bytes() > MAX_DECODED_BYTES {
        return Err(ImageError::ResourceLimit);
    }
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    let width = i32::try_from(image.width()).map_err(|_| ImageError::ResourceLimit)?;
    let height = i32::try_from(image.height()).map_err(|_| ImageError::ResourceLimit)?;
    let mut renditions = Vec::<Rendition>::new();
    for edge in [320, 640, 960] {
        let resized = if image.width().max(image.height()) <= edge {
            image.clone()
        } else {
            image.thumbnail(edge, edge)
        };
        let w = i32::try_from(resized.width()).map_err(|_| ImageError::ResourceLimit)?;
        let h = i32::try_from(resized.height()).map_err(|_| ImageError::ResourceLimit)?;
        if renditions
            .last()
            .is_some_and(|last| last.width == w && last.height == h)
        {
            continue;
        }
        let mut encoded = Vec::new();
        let kind = if resized.has_alpha() {
            resized.write_to(&mut Cursor::new(&mut encoded), ImageFormat::Png)?;
            "png"
        } else {
            JpegEncoder::new_with_quality(&mut encoded, 82).encode_image(&resized)?;
            "jpeg"
        };
        if encoded.len() > MAX_RENDITION_BYTES {
            return Err(ImageError::ResourceLimit);
        }
        renditions.push(Rendition {
            edge: i32::try_from(edge).map_err(|_| ImageError::ResourceLimit)?,
            width: w,
            height: h,
            image_type: kind,
            sha256: Sha256::digest(&encoded).into(),
            bytes: encoded,
        });
    }
    Ok(Decoded {
        registered: metadata.clone(),
        original: bytes,
        sha256: digest,
        width,
        height,
        renditions,
    })
}
pub(super) async fn run(metadata: MediaAssetDraft, bytes: Vec<u8>) -> Result<Decoded, ImageError> {
    let permit = DECODERS.try_acquire().map_err(|_| ImageError::Busy)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        prepare(&metadata, bytes)
    })
    .await
    .map_err(|_| ImageError::ProcessingUnavailable)?
}
pub(super) async fn validate_output(
    kind: String,
    width: i32,
    height: i32,
    bytes: Vec<u8>,
    digest: Vec<u8>,
) -> Result<super::PrivateRendition, ImageError> {
    let permit = DECODERS.try_acquire().map_err(|_| ImageError::Busy)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if bytes.is_empty()
            || bytes.len() > MAX_RENDITION_BYTES
            || width <= 0
            || height <= 0
            || width > 960
            || height > 960
            || Sha256::digest(&bytes)[..] != digest
        {
            return Err(ImageError::InvalidStoredData);
        }
        let format = image::guess_format(&bytes).map_err(invalid_stored_image)?;
        if !matches!(
            (kind.as_str(), format),
            ("jpeg", ImageFormat::Jpeg) | ("png", ImageFormat::Png)
        ) {
            return Err(ImageError::InvalidStoredData);
        }
        let mut decoder = decode(&bytes, format).map_err(|_| ImageError::InvalidStoredData)?;
        if decoder.dimensions()
            != (
                u32::try_from(width).map_err(|_| ImageError::InvalidStoredData)?,
                u32::try_from(height).map_err(|_| ImageError::InvalidStoredData)?,
            )
            || decoder
                .exif_metadata()
                .map_err(invalid_stored_image)?
                .is_some()
            || decoder
                .xmp_metadata()
                .map_err(invalid_stored_image)?
                .is_some()
            || decoder
                .iptc_metadata()
                .map_err(invalid_stored_image)?
                .is_some()
        {
            return Err(ImageError::InvalidStoredData);
        }
        DynamicImage::from_decoder(decoder).map_err(invalid_stored_image)?;
        Ok(super::PrivateRendition {
            image_type: kind,
            width,
            height,
            bytes,
        })
    })
    .await
    .map_err(|_| ImageError::ProcessingUnavailable)?
}

#[cfg(test)]
#[path = "../../tests/support/image_processor.rs"]
mod tests;
