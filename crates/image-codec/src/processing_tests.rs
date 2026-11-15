use crate::{
    Kind, MAX_ORIGINAL_BYTES, MAX_RENDITION_BYTES, Operation, Rejection, Response, process,
};
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
use std::io::Cursor;

fn original(kind: ImageFormat) -> Vec<u8> {
    let mut bytes = Vec::new();
    DynamicImage::new_rgba8(2, 3)
        .write_to(&mut Cursor::new(&mut bytes), kind)
        .expect("image fixture");
    bytes
}
fn jpeg() -> Vec<u8> {
    let mut bytes = Vec::new();
    DynamicImage::new_rgb8(2, 3)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .expect("JPEG fixture");
    bytes
}
fn jpeg_segment(marker: u8, payload: &[u8]) -> Vec<u8> {
    let base = jpeg();
    let mut bytes = vec![0xff, 0xd8, 0xff, marker];
    bytes.extend_from_slice(
        &u16::try_from(payload.len() + 2)
            .expect("length")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&base[2..]);
    bytes
}
fn prepare(kind: Kind, width: u32, height: u32, bytes: &[u8]) -> Response {
    process(
        Operation::Prepare {
            kind,
            width,
            height,
        },
        bytes,
    )
}
fn validate(kind: Kind, width: u32, height: u32, bytes: &[u8]) -> Response {
    process(
        Operation::Validate {
            kind,
            width,
            height,
        },
        bytes,
    )
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320_u32 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
#[test]
fn static_webp_is_accepted_and_animated_png_is_rejected() {
    let webp = original(ImageFormat::WebP);
    assert!(matches!(
        prepare(Kind::Webp, 2, 3, &webp),
        Response::Prepared(_)
    ));
    let png = original(ImageFormat::Png);
    let mut animation = b"acTL".to_vec();
    animation.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0]);
    let mut animated = png[..33].to_vec();
    animated.extend_from_slice(&8_u32.to_be_bytes());
    animated.extend_from_slice(&animation);
    animated.extend_from_slice(&crc32(&animation).to_be_bytes());
    animated.extend_from_slice(&png[33..]);
    assert_eq!(
        prepare(Kind::Png, 2, 3, &animated),
        Response::Rejected(Rejection::UnsupportedImage)
    );
}
#[test]
fn input_and_decoded_size_limits_fail_closed() {
    assert_eq!(
        prepare(Kind::Png, 2, 3, &[]),
        Response::Rejected(Rejection::ResourceLimit)
    );
    assert_eq!(
        prepare(Kind::Png, 2, 3, &vec![0; MAX_ORIGINAL_BYTES + 1]),
        Response::Rejected(Rejection::ResourceLimit)
    );
    let mut bytes = jpeg();
    let frame = bytes
        .windows(2)
        .position(|pair| pair == [0xff, 0xc0])
        .expect("baseline SOF");
    bytes[frame + 5..frame + 7].copy_from_slice(&12000_u16.to_be_bytes());
    bytes[frame + 7..frame + 9].copy_from_slice(&12000_u16.to_be_bytes());
    assert_eq!(
        prepare(Kind::Jpeg, 12000, 12000, &bytes),
        Response::Rejected(Rejection::ResourceLimit)
    );
}
#[test]
fn incompressible_alpha_rendition_hits_output_bound() {
    let mut state = 0x90ab_cdef_u32;
    let pixels = ImageBuffer::from_fn(320, 320, |_, _| {
        let mut values = [0_u8; 4];
        for value in &mut values {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *value = state as u8;
        }
        Rgba(values)
    });
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(pixels)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("fixture");
    assert!(bytes.len() > MAX_RENDITION_BYTES && bytes.len() < MAX_ORIGINAL_BYTES);
    assert_eq!(
        prepare(Kind::Png, 320, 320, &bytes),
        Response::Rejected(Rejection::ResourceLimit)
    );
}
#[test]
fn stored_dimensions_lengths_formats_and_metadata_are_checked() {
    let png = original(ImageFormat::Png);
    let invalid = Response::Rejected(Rejection::InvalidStoredData);
    for (width, height) in [(0, 3), (2, 0), (961, 3), (2, 961)] {
        assert_eq!(validate(Kind::Png, width, height, &png), invalid);
    }
    assert_eq!(
        validate(Kind::Png, 2, 3, &vec![0; MAX_RENDITION_BYTES + 1]),
        invalid
    );
    assert_eq!(validate(Kind::Jpeg, 2, 3, &png), invalid);
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x01\0\0\0\0\0\0\0";
    let iptc = b"Photoshop 3.0\0isolated-private-marker";
    assert_eq!(
        validate(Kind::Jpeg, 2, 3, &jpeg_segment(0xe1, exif)),
        invalid
    );
    assert_eq!(
        validate(Kind::Jpeg, 2, 3, &jpeg_segment(0xed, iptc)),
        invalid
    );
    assert_eq!(validate(Kind::Jpeg, 2, 3, &jpeg()), Response::Valid);
}

#[test]
fn malformed_headers_and_truncated_pixels_are_rejected() {
    let invalid = Response::Rejected(Rejection::InvalidImage);
    assert_eq!(prepare(Kind::Jpeg, 2, 3, b"\xff\xd8\xff"), invalid);
    assert_eq!(prepare(Kind::Png, 2, 3, b"\x89PNG\r\n\x1a\n"), invalid);
    let bad_webp = b"RIFF\x12\0\0\0WEBPVP8L\x05\0\0\0\x2f\x01\x80\0\xe0\0";
    assert_eq!(prepare(Kind::Webp, 2, 3, bad_webp), invalid);
    let jpeg = jpeg();
    assert_eq!(prepare(Kind::Jpeg, 2, 3, &jpeg[..jpeg.len() - 8]), invalid);
    assert_eq!(
        validate(Kind::Png, 2, 3, b"not an image"),
        Response::Rejected(Rejection::InvalidStoredData)
    );
    assert_eq!(
        validate(Kind::Png, 2, 3, b"\x89PNG\r\n\x1a\n"),
        Response::Rejected(Rejection::InvalidStoredData)
    );
}

#[test]
fn declared_dimension_limit_is_checked_before_pixels_are_decoded() {
    let mut bytes = jpeg();
    let frame = bytes
        .windows(2)
        .position(|pair| pair == [0xff, 0xc0])
        .expect("baseline SOF");
    bytes[frame + 7..frame + 9].copy_from_slice(&20000_u16.to_be_bytes());
    assert_eq!(
        prepare(Kind::Jpeg, 20000, 3, &bytes),
        Response::Rejected(Rejection::ResourceLimit)
    );
}
