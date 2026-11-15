use crate::{Kind, Operation, Rejection, Response, process, wire};
use image::{DynamicImage, ImageFormat};
use std::io::Cursor;

fn sample(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Vec::new();
    DynamicImage::new_rgb8(width, height)
        .write_to(&mut Cursor::new(&mut bytes), format)
        .expect("fixture");
    bytes
}
#[test]
fn prepare_and_validate_real_images_with_no_upscale() {
    let original = sample(1024, 768, ImageFormat::Jpeg);
    let Response::Prepared(prepared) = process(
        Operation::Prepare {
            kind: Kind::Jpeg,
            width: 1024,
            height: 768,
        },
        &original,
    ) else {
        panic!("valid JPEG");
    };
    assert_eq!((prepared.width, prepared.height), (1024, 768));
    assert_eq!(
        prepared
            .renditions
            .iter()
            .map(|r| (r.edge, r.width, r.height))
            .collect::<Vec<_>>(),
        [(320, 320, 240), (640, 640, 480), (960, 960, 720)]
    );
    for r in &prepared.renditions {
        assert_eq!(
            process(
                Operation::Validate {
                    kind: r.kind,
                    width: r.width as u32,
                    height: r.height as u32
                },
                &r.bytes
            ),
            Response::Valid
        );
    }
    let response = Response::Prepared(prepared);
    let encoded = wire::encode_response(&response).expect("encode");
    assert_eq!(wire::parse_response(&encoded), Ok(response));
    let small = sample(2, 3, ImageFormat::Png);
    let Response::Prepared(prepared) = process(
        Operation::Prepare {
            kind: Kind::Png,
            width: 2,
            height: 3,
        },
        &small,
    ) else {
        panic!("valid PNG");
    };
    assert_eq!(prepared.renditions.len(), 1);
    assert_eq!(
        (prepared.renditions[0].width, prepared.renditions[0].height),
        (2, 3)
    );
}
#[test]
fn rejects_invalid_originals_and_private_renditions() {
    let bytes = sample(2, 3, ImageFormat::Png);
    assert_eq!(
        process(
            Operation::Prepare {
                kind: Kind::Jpeg,
                width: 2,
                height: 3
            },
            &bytes
        ),
        Response::Rejected(Rejection::MetadataMismatch)
    );
    assert_eq!(
        process(
            Operation::Prepare {
                kind: Kind::Png,
                width: 3,
                height: 2
            },
            &bytes
        ),
        Response::Rejected(Rejection::MetadataMismatch)
    );
    assert_eq!(
        process(
            Operation::Prepare {
                kind: Kind::Png,
                width: 2,
                height: 3
            },
            b"not an image"
        ),
        Response::Rejected(Rejection::UnsupportedImage)
    );
    assert_eq!(
        process(
            Operation::Validate {
                kind: Kind::Png,
                width: 3,
                height: 2
            },
            &bytes
        ),
        Response::Rejected(Rejection::InvalidStoredData)
    );
    assert_eq!(
        process(
            Operation::Validate {
                kind: Kind::Webp,
                width: 2,
                height: 3
            },
            &bytes
        ),
        Response::Rejected(Rejection::InvalidStoredData)
    );
    assert_eq!(
        process(
            Operation::Validate {
                kind: Kind::Png,
                width: 2,
                height: 3
            },
            &[]
        ),
        Response::Rejected(Rejection::InvalidStoredData)
    );
}
#[test]
fn private_jpeg_metadata_is_rejected() {
    let base = sample(2, 3, ImageFormat::Jpeg);
    let payload =
        b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta xmlns:x='adobe:ns:meta/'>private</x:xmpmeta>";
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
    bytes.extend_from_slice(
        &u16::try_from(payload.len() + 2)
            .expect("length")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&base[2..]);
    assert_eq!(
        process(
            Operation::Validate {
                kind: Kind::Jpeg,
                width: 2,
                height: 3
            },
            &bytes
        ),
        Response::Rejected(Rejection::InvalidStoredData)
    );
}
#[test]
fn wire_rejects_corruption_and_truncation() {
    let op = Operation::Prepare {
        kind: Kind::Jpeg,
        width: 1024,
        height: 768,
    };
    let header = wire::request_header(op, 22).expect("header");
    assert_eq!(wire::parse_request_header(&header), Ok((op, 22)));
    for (offset, value) in [(0, b'X'), (4, 3), (5, 9), (6, 1), (8, 255), (16, 255)] {
        let mut corrupt = header;
        corrupt[offset] = value;
        assert!(
            wire::parse_request_header(&corrupt).is_err(),
            "offset {offset}"
        );
    }
    assert!(wire::request_header(op, 0).is_err());
    let reject =
        wire::encode_response(&Response::Rejected(Rejection::Unavailable)).expect("encode");
    assert_eq!(
        wire::parse_response(&reject),
        Ok(Response::Rejected(Rejection::Unavailable))
    );
    for length in 0..reject.len() {
        assert!(wire::parse_response(&reject[..length]).is_err());
    }
    for corrupt in [
        b"ICR1\x01\x07\x00\x00".as_slice(),
        b"ICR1\x00\x00\x00\x00x",
        b"ICR1\x02\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\x01\xff\xff\xff\xff",
    ] {
        assert!(wire::parse_response(corrupt).is_err());
    }
}

#[test]
fn wire_rejects_forged_prepared_shape_and_declared_lengths() {
    let bytes = sample(640, 480, ImageFormat::Jpeg);
    let response = process(
        Operation::Prepare {
            kind: Kind::Jpeg,
            width: 640,
            height: 480,
        },
        &bytes,
    );
    let encoded = wire::encode_response(&response).expect("valid prepared");
    for length in 0..encoded.len() {
        assert!(wire::parse_response(&encoded[..length]).is_err());
    }
    for (offset, value) in [(8, 255), (19, 3), (23, 1), (35, 3), (36, 1)] {
        let mut corrupt = encoded.clone();
        corrupt[offset] = value;
        assert!(wire::parse_response(&corrupt).is_err(), "offset {offset}");
    }
    let mut with_trailing = encoded;
    with_trailing.push(0);
    assert!(wire::parse_response(&with_trailing).is_err());
    let Response::Prepared(mut prepared) = response else {
        panic!("prepared");
    };
    prepared.renditions[0].edge = 640;
    assert!(wire::encode_response(&Response::Prepared(prepared)).is_err());
}

#[test]
fn derivatives_strip_original_metadata() {
    let base = sample(1024, 768, ImageFormat::Jpeg);
    let payload = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta xmlns:x='adobe:ns:meta/'>private-marker</x:xmpmeta>";
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
    bytes.extend_from_slice(
        &u16::try_from(payload.len() + 2)
            .expect("length")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&base[2..]);
    let Response::Prepared(prepared) = process(
        Operation::Prepare {
            kind: Kind::Jpeg,
            width: 1024,
            height: 768,
        },
        &bytes,
    ) else {
        panic!("valid image");
    };
    for rendition in prepared.renditions {
        assert!(
            !rendition
                .bytes
                .windows(b"private-marker".len())
                .any(|window| window == b"private-marker")
        );
        assert_eq!(
            process(
                Operation::Validate {
                    kind: rendition.kind,
                    width: u32::try_from(rendition.width).expect("positive"),
                    height: u32::try_from(rendition.height).expect("positive")
                },
                &rendition.bytes
            ),
            Response::Valid
        );
    }
}
