use crate::{
    Kind, MAX_ORIGINAL_BYTES, MAX_RENDITION_BYTES, Prepared, Rejection, Rendition, framing,
};
use image::{
    DynamicImage, ImageDecoder, ImageFormat, Limits,
    codecs::{
        jpeg::{JpegDecoder, JpegEncoder},
        png::PngDecoder,
        webp::WebPDecoder,
    },
};
use std::io::Cursor;

const MAX_DECODED_BYTES: u64 = 128 * 1024 * 1024;
fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    limits
}
fn decode<'a>(bytes: &'a [u8], kind: Kind) -> Result<Box<dyn ImageDecoder + 'a>, Rejection> {
    let mut decoder: Box<dyn ImageDecoder> = match kind {
        Kind::Jpeg => {
            Box::new(JpegDecoder::new(Cursor::new(bytes)).map_err(|_| Rejection::InvalidImage)?)
        }
        Kind::Png => {
            let decoder = PngDecoder::with_limits(Cursor::new(bytes), limits())
                .map_err(|_| Rejection::InvalidImage)?;
            if decoder.is_apng().map_err(|_| Rejection::InvalidImage)? {
                return Err(Rejection::UnsupportedImage);
            }
            Box::new(decoder)
        }
        Kind::Webp => {
            framing::webp(bytes)?;
            let decoder =
                WebPDecoder::new(Cursor::new(bytes)).map_err(|_| Rejection::InvalidImage)?;
            if decoder.has_animation() {
                return Err(Rejection::UnsupportedImage);
            }
            Box::new(decoder)
        }
    };
    decoder
        .set_limits(limits())
        .map_err(|_| Rejection::ResourceLimit)?;
    Ok(decoder)
}
fn format(kind: Kind) -> ImageFormat {
    match kind {
        Kind::Jpeg => ImageFormat::Jpeg,
        Kind::Png => ImageFormat::Png,
        Kind::Webp => ImageFormat::WebP,
    }
}
pub(crate) fn prepare(
    kind: Kind,
    width: u32,
    height: u32,
    bytes: &[u8],
) -> Result<Prepared, Rejection> {
    if bytes.is_empty() || bytes.len() > MAX_ORIGINAL_BYTES {
        return Err(Rejection::ResourceLimit);
    }
    let detected = image::guess_format(bytes).map_err(|_| Rejection::UnsupportedImage)?;
    if detected != format(kind) {
        return Err(Rejection::MetadataMismatch);
    }
    let mut decoder = decode(bytes, kind)?;
    if decoder.dimensions() != (width, height) {
        return Err(Rejection::MetadataMismatch);
    }
    if decoder.total_bytes() > MAX_DECODED_BYTES {
        return Err(Rejection::ResourceLimit);
    }
    let orientation = decoder.orientation().map_err(|_| Rejection::InvalidImage)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|_| Rejection::InvalidImage)?;
    image.apply_orientation(orientation);
    let width = i32::try_from(image.width()).map_err(|_| Rejection::ResourceLimit)?;
    let height = i32::try_from(image.height()).map_err(|_| Rejection::ResourceLimit)?;
    let mut renditions: Vec<Rendition> = Vec::new();
    for edge in [320_u32, 640, 960] {
        let resized = if image.width().max(image.height()) <= edge {
            image.clone()
        } else {
            image.thumbnail(edge, edge)
        };
        let w = i32::try_from(resized.width()).map_err(|_| Rejection::ResourceLimit)?;
        let h = i32::try_from(resized.height()).map_err(|_| Rejection::ResourceLimit)?;
        if renditions
            .last()
            .is_some_and(|last| last.width == w && last.height == h)
        {
            continue;
        }
        let mut encoded = Vec::new();
        let kind = if resized.has_alpha() {
            resized
                .write_to(&mut Cursor::new(&mut encoded), ImageFormat::Png)
                .map_err(|_| Rejection::InvalidImage)?;
            Kind::Png
        } else {
            JpegEncoder::new_with_quality(&mut encoded, 82)
                .encode_image(&resized)
                .map_err(|_| Rejection::InvalidImage)?;
            Kind::Jpeg
        };
        if encoded.len() > MAX_RENDITION_BYTES {
            return Err(Rejection::ResourceLimit);
        }
        renditions.push(Rendition {
            edge: i32::try_from(edge).map_err(|_| Rejection::ResourceLimit)?,
            width: w,
            height: h,
            kind,
            bytes: encoded,
        });
    }
    Ok(Prepared {
        width,
        height,
        renditions,
    })
}
pub(crate) fn validate(kind: Kind, width: u32, height: u32, bytes: &[u8]) -> Result<(), Rejection> {
    let invalid = || Rejection::InvalidStoredData;
    if bytes.is_empty()
        || bytes.len() > MAX_RENDITION_BYTES
        || width == 0
        || height == 0
        || width > 960
        || height > 960
        || kind == Kind::Webp
    {
        return Err(invalid());
    }
    if image::guess_format(bytes).map_err(|_| invalid())? != format(kind) {
        return Err(invalid());
    }
    let mut decoder = decode(bytes, kind).map_err(|_| invalid())?;
    if decoder.dimensions() != (width, height)
        || decoder.exif_metadata().map_err(|_| invalid())?.is_some()
        || decoder.xmp_metadata().map_err(|_| invalid())?.is_some()
        || decoder.iptc_metadata().map_err(|_| invalid())?.is_some()
    {
        return Err(invalid());
    }
    DynamicImage::from_decoder(decoder).map_err(|_| invalid())?;
    Ok(())
}
