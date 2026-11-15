use super::*;
use commerce_image_codec::{Kind, Operation, Rejection, Response};
use image::{DynamicImage, ImageFormat};
use std::io::Cursor;
static CASES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(unix)]
#[tokio::test]
async fn configured_processing_failure_does_not_decode_locally() {
    let _case = CASES.lock().await;
    let endpoint = transport::DecoderEndpoint::parse(&format!(
        "/sark-absent-decoder-{}/decoder.sock",
        std::process::id()
    ))
    .expect("closed configuration");
    let operation = Operation::Prepare {
        kind: Kind::Png,
        width: 2,
        height: 3,
    };
    let mut bytes = Vec::new();
    DynamicImage::new_rgba8(2, 3)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("valid fixture for the local codec");
    let permit = DECODERS.try_acquire().expect("available");
    assert!(matches!(
        run_codec(None, operation, bytes.clone(), permit).await,
        Ok((Response::Prepared(_), _))
    ));
    let permit = DECODERS.try_acquire().expect("available");
    assert!(matches!(
        run_codec(Some(&endpoint), operation, bytes, permit).await,
        Err(ImageError::ProcessingUnavailable)
    ));
}

#[test]
fn image_reply_must_match_the_original_and_support_only_safe_rendition_types() {
    let metadata = MediaAssetDraft {
        sha256: [0; 32],
        image_type: ImageType::Png,
        width_px: 2,
        height_px: 3,
        kind: commerce_domain::media::MediaKind::OriginalPhoto,
        fixture: true,
        source_ref: "synthetic image custody".into(),
    };
    for (width, height, kind) in [(99, 3, Kind::Png), (3, 2, Kind::Webp)] {
        let reply = Prepared {
            width,
            height,
            renditions: vec![commerce_image_codec::Rendition {
                edge: 320,
                width,
                height,
                kind,
                bytes: vec![1, 2, 3],
            }],
        };
        assert!(matches!(
            assemble(metadata.clone(), vec![], [0; 32], reply),
            Err(ImageError::InvalidStoredData)
        ));
    }
}

#[tokio::test]
async fn saturated_decoder_capacity_rejects_without_queueing() {
    let _case = CASES.lock().await;
    let permits = DECODERS.acquire_many(2).await.expect("capacity");
    let metadata = MediaAssetDraft {
        sha256: [0; 32],
        image_type: ImageType::Png,
        width_px: 1,
        height_px: 1,
        kind: commerce_domain::media::MediaKind::OriginalPhoto,
        fixture: true,
        source_ref: "isolated decoder capacity".into(),
    };
    assert!(matches!(
        run(metadata, vec![1]).await,
        Err(ImageError::Busy)
    ));
    assert!(matches!(
        validate_output("png".into(), 1, 1, vec![1], vec![0; 32]).await,
        Err(ImageError::Busy)
    ));
    drop(permits);
}
#[tokio::test]
async fn decoder_dimension_and_stored_byte_boundaries_fail_closed() {
    let _case = CASES.lock().await;
    let metadata = MediaAssetDraft {
        sha256: [0; 32],
        image_type: ImageType::Png,
        width_px: 1,
        height_px: 1,
        kind: commerce_domain::media::MediaKind::OriginalPhoto,
        fixture: true,
        source_ref: "isolated boundary".into(),
    };
    let mut oversized_header = Vec::new();
    DynamicImage::new_rgb8(2, 3)
        .write_to(&mut Cursor::new(&mut oversized_header), ImageFormat::Jpeg)
        .expect("JPEG header fixture");
    let frame = oversized_header
        .windows(2)
        .position(|pair| pair == [0xff, 0xc0])
        .expect("baseline SOF");
    oversized_header[frame + 7..frame + 9].copy_from_slice(&20000_u16.to_be_bytes());
    assert!(matches!(
        commerce_image_codec::process(
            Operation::Prepare {
                kind: Kind::Jpeg,
                width: 2,
                height: 3
            },
            &oversized_header,
        ),
        Response::Rejected(Rejection::ResourceLimit)
    ));
    let mut bytes = Vec::new();
    DynamicImage::new_rgba8(2, 3)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("actual fixture");
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let registered = MediaAssetDraft {
        sha256: digest,
        width_px: 2,
        height_px: 3,
        ..metadata
    };
    let prepared = run(registered, bytes.clone())
        .await
        .expect("valid original");
    assert_eq!((prepared.width, prepared.height), (2, 3));
    assert_eq!(prepared.renditions.len(), 1);
    let verified = validate_output("png".into(), 2, 3, bytes.clone(), digest.to_vec())
        .await
        .expect("metadata-free rendition");
    assert_eq!(verified.bytes, bytes);
    for case in 0..10 {
        let mut original = bytes.clone();
        let mut kind = "png".to_owned();
        let mut width = 2;
        let mut height = 3;
        match case {
            0 => original.clear(),
            1 => original = vec![0; MAX_RENDITION_BYTES + 1],
            2 => width = 0,
            3 => height = 0,
            4 => width = 961,
            5 => height = 961,
            6 => kind = "jpeg".into(),
            7 => width = 3,
            8 => original = b"\x89PNG\r\n\x1a\n".to_vec(),
            9 => original = b"corrupt private image signature".to_vec(),
            _ => unreachable!(),
        }
        let digest = Sha256::digest(&original).to_vec();
        assert!(
            matches!(
                validate_output(kind, width, height, original, digest).await,
                Err(ImageError::InvalidStoredData)
            ),
            "case {case}"
        );
    }
    assert!(matches!(
        validate_output("png".into(), 2, 3, bytes, vec![0; 32]).await,
        Err(ImageError::InvalidStoredData)
    ));
}

#[tokio::test]
async fn stored_jpeg_privacy_metadata_is_rejected_even_with_a_matching_digest() {
    let _case = CASES.lock().await;
    let mut base = Vec::new();
    DynamicImage::new_rgb8(2, 3)
        .write_to(&mut Cursor::new(&mut base), ImageFormat::Jpeg)
        .expect("JPEG fixture");
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x01\0\0\0\0\0\0\0";
    let xmp = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta xmlns:x='adobe:ns:meta/'>isolated-private-marker</x:xmpmeta>";
    let iptc = b"Photoshop 3.0\0isolated-private-marker";
    for (marker, payload) in [(0xe1, exif.as_slice()), (0xe1, xmp), (0xed, iptc)] {
        let mut bytes = vec![0xff, 0xd8, 0xff, marker];
        bytes.extend(
            u16::try_from(payload.len() + 2)
                .expect("segment length")
                .to_be_bytes(),
        );
        bytes.extend(payload);
        bytes.extend_from_slice(&base[2..]);
        let digest = Sha256::digest(&bytes).to_vec();
        assert!(matches!(
            validate_output("jpeg".into(), 2, 3, bytes, digest).await,
            Err(ImageError::InvalidStoredData)
        ));
    }
}

#[tokio::test]
async fn original_xmp_and_iptc_are_removed_from_every_large_image_derivative() {
    let _case = CASES.lock().await;
    let mut base = Vec::new();
    DynamicImage::new_rgb8(1024, 768)
        .write_to(&mut Cursor::new(&mut base), ImageFormat::Jpeg)
        .expect("JPEG fixture");
    let xmp = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta xmlns:x='adobe:ns:meta/'>isolated-private-marker</x:xmpmeta>";
    let iptc = b"Photoshop 3.0\0isolated-private-marker";
    let mut original = vec![0xff, 0xd8];
    for (marker, payload) in [(0xe1, xmp.as_slice()), (0xed, iptc)] {
        original.extend([0xff, marker]);
        original.extend(
            u16::try_from(payload.len() + 2)
                .expect("segment length")
                .to_be_bytes(),
        );
        original.extend(payload);
    }
    original.extend_from_slice(&base[2..]);
    let metadata = MediaAssetDraft {
        sha256: Sha256::digest(&original).into(),
        image_type: ImageType::Jpeg,
        width_px: 1024,
        height_px: 768,
        kind: commerce_domain::media::MediaKind::OriginalPhoto,
        fixture: true,
        source_ref: "isolated private metadata".into(),
    };
    let result = run(metadata, original)
        .await
        .expect("valid private original");
    assert_eq!(
        result
            .renditions
            .iter()
            .map(|r| (r.width, r.height))
            .collect::<Vec<_>>(),
        [(320, 240), (640, 480), (960, 720)]
    );
    for r in result.renditions {
        assert!(
            !r.bytes
                .windows(b"isolated-private-marker".len())
                .any(|part| part == b"isolated-private-marker")
        );
        validate_output(
            r.image_type.into(),
            r.width,
            r.height,
            r.bytes,
            r.sha256.to_vec(),
        )
        .await
        .expect("no personal metadata remains");
    }
}
