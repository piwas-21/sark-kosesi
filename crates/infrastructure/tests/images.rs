use commerce_application::request_context::{Clock, ContextError};
use commerce_domain::media::{ImageType, MediaAssetDraft, MediaKind};
use commerce_infrastructure::{
    MIGRATOR,
    image_ingestion::{
        ImageError, IngestImageRequest, MAX_ORIGINAL_BYTES, image_receipt, ingest_image,
        private_rendition,
    },
    media::{RegisterMediaRequest, register_media},
    staff_access::{SessionToken, StaffError},
};
use image::{DynamicImage, ImageDecoder, ImageFormat};
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgPool};
use std::io::Cursor;
static CASES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
struct FixedClock(u64);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
fn encoded(format: ImageFormat, width: u32, height: u32) -> Vec<u8> {
    let image = if format == ImageFormat::Png {
        DynamicImage::new_rgba8(width, height)
    } else {
        DynamicImage::new_rgb8(width, height)
    };
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), format)
        .expect("isolated encoded pixels");
    bytes
}
fn metadata(bytes: &[u8], image_type: ImageType, width_px: u32, height_px: u32) -> MediaAssetDraft {
    MediaAssetDraft {
        sha256: Sha256::digest(bytes).into(),
        image_type,
        width_px,
        height_px,
        kind: MediaKind::OriginalPhoto,
        fixture: true,
        source_ref: "isolated synthetic image fixture".into(),
    }
}
fn request<'a>(asset_id: &'a str, bytes: &'a [u8], effect_key: &'a str) -> IngestImageRequest<'a> {
    IngestImageRequest {
        asset_id,
        bytes,
        effect_key,
        reason: "isolated image ingestion",
        confirmed: true,
    }
}
async fn seed(pool: &PgPool) {
    pool.execute("INSERT INTO staff_accounts VALUES ('fixture-catalogue',true),('fixture-warehouse',true),('fixture-other',true); INSERT INTO staff_capabilities VALUES ('fixture-catalogue','catalogue.manage'),('fixture-warehouse','warehouse.manage'),('fixture-other','catalogue.manage')").await.expect("fixture grants");
    for (number, actor) in [
        (1, "fixture-catalogue"),
        (2, "fixture-warehouse"),
        (3, "fixture-other"),
    ] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,4600,1100)").bind(SessionToken::from_bytes([number;32]).digest().as_slice()).bind(actor).execute(pool).await.expect("test-only session");
    }
}
async fn register(pool: &PgPool, id: &str, draft: &MediaAssetDraft) {
    register_media(
        pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        RegisterMediaRequest {
            asset_id: id,
            effect_key: id,
            draft,
            reason: "isolated synthetic image",
            confirmed: true,
        },
    )
    .await
    .expect("draft registry");
}
async fn counts(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM catalogue_image_originals),(SELECT count(*) FROM catalogue_image_renditions)").fetch_one(pool).await.expect("counts")
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn actual_jpeg_png_webp_bytes_produce_private_bounded_atomic_images(pool: PgPool) {
    let _case = CASES.lock().await;
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    for (id, format, kind) in [
        ("jpeg", ImageFormat::Jpeg, ImageType::Jpeg),
        ("png", ImageFormat::Png, ImageType::Png),
        ("webp", ImageFormat::WebP, ImageType::Webp),
    ] {
        let bytes = encoded(format, 1024, 768);
        register(&pool, id, &metadata(&bytes, kind, 1024, 768)).await;
        let receipt = ingest_image(&pool, &token, &FixedClock(1100), request(id, &bytes, id))
            .await
            .expect("decode/store");
        assert_eq!(
            (
                receipt.display_width,
                receipt.display_height,
                receipt.recipe
            ),
            (1024, 768, "image-v1")
        );
        assert!(!receipt.replayed);
        assert_eq!(
            receipt
                .renditions
                .iter()
                .map(|r| (r.edge, r.width, r.height))
                .collect::<Vec<_>>(),
            vec![(320, 320, 240), (640, 640, 480), (960, 960, 720)]
        );
        for rendition in &receipt.renditions {
            let private = private_rendition(&pool, &token, &FixedClock(1100), id, rendition.edge)
                .await
                .expect("guarded private read")
                .expect("rendition");
            assert!(private.bytes.len() <= 256000);
            assert_eq!(&Sha256::digest(&private.bytes)[..], rendition.sha256);
            let reader = image::ImageReader::new(Cursor::new(&private.bytes))
                .with_guessed_format()
                .expect("guess");
            assert_eq!(
                reader.into_dimensions().expect("actual dimensions"),
                (
                    u32::try_from(rendition.width).expect("width"),
                    u32::try_from(rendition.height).expect("height")
                )
            );
        }
        let replay = ingest_image(&pool, &token, &FixedClock(1100), request(id, &bytes, id))
            .await
            .expect("retry");
        assert!(replay.replayed);
        assert_eq!(replay.renditions, receipt.renditions);
        assert_eq!(
            image_receipt(&pool, &token, &FixedClock(1100), id)
                .await
                .expect("receipt read")
                .expect("stored")
                .renditions,
            receipt.renditions
        );
    }
    assert_eq!(counts(&pool).await, (3, 9));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn tiny_images_are_not_upscaled_and_absent_private_reads_are_bounded(pool: PgPool) {
    let _case = CASES.lock().await;
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let bytes = encoded(ImageFormat::Png, 2, 3);
    register(&pool, "tiny", &metadata(&bytes, ImageType::Png, 2, 3)).await;
    let receipt = ingest_image(
        &pool,
        &token,
        &FixedClock(1100),
        request("tiny", &bytes, "tiny"),
    )
    .await
    .expect("tiny image");
    assert_eq!(receipt.renditions.len(), 1);
    assert_eq!(
        (receipt.renditions[0].width, receipt.renditions[0].height),
        (2, 3)
    );
    assert_eq!(
        image_receipt(&pool, &token, &FixedClock(1100), "missing")
            .await
            .expect("missing"),
        None
    );
    assert!(
        private_rendition(&pool, &token, &FixedClock(1100), "tiny", 960)
            .await
            .expect("missing edge")
            .is_none()
    );
    assert_eq!(counts(&pool).await, (1, 1));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn authority_confirmation_envelope_and_mismatched_bytes_cannot_be_verified(pool: PgPool) {
    let _case = CASES.lock().await;
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let bytes = encoded(ImageFormat::Png, 2, 3);
    register(&pool, "asset", &metadata(&bytes, ImageType::Png, 2, 3)).await;
    for number in [2, 99] {
        let wrong = SessionToken::from_bytes([number; 32]);
        assert!(matches!(
            ingest_image(
                &pool,
                &wrong,
                &FixedClock(1100),
                request("asset", &bytes, "denied")
            )
            .await,
            Err(ImageError::Staff(StaffError::Denied))
        ));
        assert!(matches!(
            image_receipt(&pool, &wrong, &FixedClock(1100), "asset").await,
            Err(ImageError::Staff(StaffError::Denied))
        ));
        assert!(matches!(
            private_rendition(&pool, &wrong, &FixedClock(1100), "asset", 320).await,
            Err(ImageError::Staff(StaffError::Denied))
        ));
    }
    for case in 0..7 {
        let mut r = request("asset", &bytes, "invalid");
        let long = "x".repeat(501);
        let huge = vec![0; MAX_ORIGINAL_BYTES + 1];
        match case {
            0 => r.confirmed = false,
            1 => r.asset_id = "../",
            2 => r.effect_key = "",
            3 => r.reason = " ",
            4 => r.reason = &long,
            5 => r.bytes = &[],
            6 => r.bytes = &huge,
            _ => unreachable!(),
        }
        assert!(
            matches!(
                ingest_image(&pool, &token, &FixedClock(1100), r).await,
                Err(ImageError::InvalidRequest)
            ),
            "case {case}"
        );
    }
    assert!(matches!(
        ingest_image(
            &pool,
            &token,
            &FixedClock(1100),
            request("missing", &bytes, "missing")
        )
        .await,
        Err(ImageError::NotFound)
    ));
    assert!(matches!(
        image_receipt(&pool, &token, &FixedClock(1100), "../").await,
        Err(ImageError::InvalidRequest)
    ));
    for (id, edge) in [("../", 320), ("asset", 1)] {
        assert!(matches!(
            private_rendition(&pool, &token, &FixedClock(1100), id, edge).await,
            Err(ImageError::InvalidRequest)
        ));
    }
    let changed = encoded(ImageFormat::Png, 3, 3);
    assert!(matches!(
        ingest_image(
            &pool,
            &token,
            &FixedClock(1100),
            request("asset", &changed, "wrong-hash")
        )
        .await,
        Err(ImageError::MetadataMismatch)
    ));
    let mut wrong = metadata(&changed, ImageType::Jpeg, 3, 3);
    register(&pool, "codec", &wrong).await;
    assert!(matches!(
        ingest_image(
            &pool,
            &token,
            &FixedClock(1100),
            request("codec", &changed, "wrong-codec")
        )
        .await,
        Err(ImageError::MetadataMismatch)
    ));
    let dimension_bytes = encoded(ImageFormat::Jpeg, 4, 4);
    wrong = metadata(&dimension_bytes, ImageType::Jpeg, 5, 4);
    register(&pool, "dimensions", &wrong).await;
    assert!(matches!(
        ingest_image(
            &pool,
            &token,
            &FixedClock(1100),
            request("dimensions", &dimension_bytes, "wrong-dimensions")
        )
        .await,
        Err(ImageError::MetadataMismatch)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut body = Vec::from(kind.as_slice());
    body.extend_from_slice(data);
    let mut crc = 0xffff_ffff_u32;
    for byte in &body {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    let mut chunk = u32::try_from(data.len())
        .expect("bounded fixture")
        .to_be_bytes()
        .to_vec();
    chunk.extend(body);
    chunk.extend((!crc).to_be_bytes());
    chunk
}
fn riff_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut result = kind.to_vec();
    result.extend(u32::try_from(data.len()).expect("bounded").to_le_bytes());
    result.extend(data);
    if data.len() % 2 == 1 {
        result.push(0);
    }
    result
}
fn animated_webp() -> Vec<u8> {
    let static_image = encoded(ImageFormat::WebP, 2, 3);
    let extended = [2, 0, 0, 0, 1, 0, 0, 2, 0, 0];
    let animation = [0; 6];
    let mut frame = vec![0; 6];
    frame.extend([1, 0, 0, 2, 0, 0, 100, 0, 0, 0]);
    frame.extend_from_slice(&static_image[12..]);
    let mut payload = b"WEBP".to_vec();
    payload.extend(riff_chunk(b"VP8X", &extended));
    payload.extend(riff_chunk(b"ANIM", &animation));
    payload.extend(riff_chunk(b"ANMF", &frame));
    let mut result = b"RIFF".to_vec();
    result.extend(u32::try_from(payload.len()).expect("bounded").to_le_bytes());
    result.extend(payload);
    result
}
fn extended_webp_with_exif() -> Vec<u8> {
    let image = encoded(ImageFormat::WebP, 2, 3);
    // TIFF orientation6. WebP EXIF carries the TIFF bytes without JPEG's prefix.
    let exif = [
        b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
    ];
    let mut payload = b"WEBP".to_vec();
    payload.extend(riff_chunk(b"VP8X", &[8, 0, 0, 0, 1, 0, 0, 2, 0, 0]));
    payload.extend_from_slice(&image[12..]);
    payload.extend(riff_chunk(b"EXIF", &exif));
    let mut bytes = b"RIFF".to_vec();
    bytes.extend(u32::try_from(payload.len()).expect("fixture").to_le_bytes());
    bytes.extend(payload);
    bytes
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn webp_metadata_ranges_are_bounded_before_processing_and_valid_orientation_is_preserved(
    pool: PgPool,
) {
    let _case = CASES.lock().await;
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let mut invalid = extended_webp_with_exif();
    let exif = invalid
        .windows(4)
        .position(|part| part == b"EXIF")
        .expect("metadata chunk");
    invalid[exif + 4..exif + 8].copy_from_slice(&65536_u32.to_le_bytes());
    register(
        &pool,
        "range-bomb",
        &metadata(&invalid, ImageType::Webp, 2, 3),
    )
    .await;
    assert!(matches!(
        ingest_image(
            &pool,
            &token,
            &FixedClock(1100),
            request("range-bomb", &invalid, "range-bomb")
        )
        .await,
        Err(ImageError::ResourceLimit)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    let valid = extended_webp_with_exif();
    register(
        &pool,
        "valid-webp-exif",
        &metadata(&valid, ImageType::Webp, 2, 3),
    )
    .await;
    let receipt = ingest_image(
        &pool,
        &token,
        &FixedClock(1100),
        request("valid-webp-exif", &valid, "valid-webp-exif"),
    )
    .await
    .expect("actual WebP orientation");
    assert_eq!((receipt.display_width, receipt.display_height), (3, 2));
    let output = private_rendition(&pool, &token, &FixedClock(1100), "valid-webp-exif", 320)
        .await
        .expect("metadata-free read")
        .expect("rendition");
    assert_eq!((output.width, output.height), (3, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn unsupported_animated_truncated_and_header_bombs_never_create_image_records(pool: PgPool) {
    let _case = CASES.lock().await;
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let svg = b"<svg xmlns='http://www.w3.org/2000/svg'><script>fixture()</script></svg>".to_vec();
    let truncated = b"\x89PNG\r\n\x1a\n".to_vec();
    for (id, bytes, expected_unsupported) in [("svg", svg, true), ("truncated", truncated, false)] {
        register(&pool, id, &metadata(&bytes, ImageType::Png, 2, 3)).await;
        let error = ingest_image(&pool, &token, &FixedClock(1100), request(id, &bytes, id))
            .await
            .expect_err("malformed image");
        if expected_unsupported {
            assert!(matches!(error, ImageError::UnsupportedImage));
        } else {
            assert!(matches!(error, ImageError::InvalidImage));
        }
    }
    let original = encoded(ImageFormat::Png, 2, 3);
    let mut apng = original[..33].to_vec();
    apng.extend(png_chunk(b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]));
    apng.extend_from_slice(&original[33..]);
    for (id, bytes, kind) in [
        ("apng", apng, ImageType::Png),
        ("animated-webp", animated_webp(), ImageType::Webp),
    ] {
        register(&pool, id, &metadata(&bytes, kind, 2, 3)).await;
        assert!(
            matches!(
                ingest_image(&pool, &token, &FixedClock(1100), request(id, &bytes, id)).await,
                Err(ImageError::UnsupportedImage)
            ),
            "animated {id}"
        );
    }
    let mut bomb = encoded(ImageFormat::Jpeg, 2, 3);
    let marker = bomb
        .windows(2)
        .position(|w| w == [0xff, 0xc0])
        .expect("SOF0 fixture");
    bomb[marker + 5..marker + 7].copy_from_slice(&8192_u16.to_be_bytes());
    bomb[marker + 7..marker + 9].copy_from_slice(&8192_u16.to_be_bytes());
    register(&pool, "bomb", &metadata(&bomb, ImageType::Jpeg, 8192, 8192)).await;
    assert!(matches!(
        ingest_image(
            &pool,
            &token,
            &FixedClock(1100),
            request("bomb", &bomb, "bomb")
        )
        .await,
        Err(ImageError::ResourceLimit)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn orientation_is_normalized_and_personal_metadata_stays_only_in_private_original(
    pool: PgPool,
) {
    let _case = CASES.lock().await;
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let base = encoded(ImageFormat::Jpeg, 2, 3);
    // Little-endian TIFF orientation6, independently specified, plus a private COM.
    let exif = [
        b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0,
        0, 6, 0, 0, 0, 0, 0, 0, 0,
    ];
    let private = b"ISOLATED-PRIVATE-COMMENT-NOT-FOR-DERIVATIVES";
    let mut source = vec![0xff, 0xd8, 0xff, 0xe1];
    source.extend(
        u16::try_from(exif.len() + 2)
            .expect("EXIF length")
            .to_be_bytes(),
    );
    source.extend(exif);
    source.extend([0xff, 0xfe]);
    source.extend(
        u16::try_from(private.len() + 2)
            .expect("COM length")
            .to_be_bytes(),
    );
    source.extend(private);
    source.extend_from_slice(&base[2..]);
    register(&pool, "oriented", &metadata(&source, ImageType::Jpeg, 2, 3)).await;
    let result = ingest_image(
        &pool,
        &token,
        &FixedClock(1100),
        request("oriented", &source, "oriented"),
    )
    .await
    .expect("orientation");
    assert_eq!((result.display_width, result.display_height), (3, 2));
    let output = private_rendition(&pool, &token, &FixedClock(1100), "oriented", 320)
        .await
        .expect("private read")
        .expect("image");
    assert!(
        !output
            .bytes
            .windows(private.len())
            .any(|chunk| chunk == private)
    );
    let mut decoder =
        image::codecs::jpeg::JpegDecoder::new(Cursor::new(&output.bytes)).expect("actual JPEG");
    assert_eq!(decoder.dimensions(), (3, 2));
    assert_eq!(decoder.exif_metadata().expect("metadata"), None);
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT bytes FROM catalogue_image_originals WHERE asset_id='oriented'")
            .fetch_one(&pool)
            .await
            .expect("original storage");
    assert_eq!(stored, source);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn image_effects_are_intent_bound_and_concurrent_retries_do_not_duplicate_storage(
    pool: PgPool,
) {
    let _case = CASES.lock().await;
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let clock = FixedClock(1100);
    let bytes = encoded(ImageFormat::Png, 2, 3);
    register(&pool, "one", &metadata(&bytes, ImageType::Png, 2, 3)).await;
    let (a, b) = tokio::join!(
        ingest_image(&pool, &token, &clock, request("one", &bytes, "same")),
        ingest_image(&pool, &token, &clock, request("one", &bytes, "same"))
    );
    let a = a.expect("write/replay");
    let b = b.expect("write/replay");
    assert_ne!(a.replayed, b.replayed);
    assert_eq!(counts(&pool).await, (1, 1));
    for case in 0..3 {
        let mut r = request("one", &bytes, "same");
        let other = SessionToken::from_bytes([3; 32]);
        let actor = if case == 1 { &other } else { &token };
        match case {
            0 => r.reason = "different",
            1 => {}
            2 => r.effect_key = "fresh-key",
            _ => unreachable!(),
        }
        assert!(matches!(
            ingest_image(&pool, actor, &clock, r).await,
            Err(ImageError::Conflict)
        ));
    }
    let other_bytes = encoded(ImageFormat::Jpeg, 3, 3);
    register(&pool, "two", &metadata(&other_bytes, ImageType::Jpeg, 3, 3)).await;
    assert!(matches!(
        ingest_image(&pool, &token, &clock, request("two", &other_bytes, "same")).await,
        Err(ImageError::Conflict)
    ));
    sqlx::query("UPDATE staff_sessions SET revoked_at=1200 WHERE token_hash=$1")
        .bind(token.digest().as_slice())
        .execute(&pool)
        .await
        .expect("revoke");
    assert!(matches!(
        ingest_image(
            &pool,
            &token,
            &FixedClock(1200),
            request("one", &bytes, "same")
        )
        .await,
        Err(ImageError::Staff(StaffError::Denied))
    ));
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authority_is_rechecked_after_processing_and_before_committing_or_returning_bytes(
    pool: PgPool,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct StepClock {
        calls: AtomicUsize,
        fail_at: usize,
        unavailable: bool,
    }
    impl Clock for StepClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) >= self.fail_at {
                if self.unavailable {
                    Err(ContextError::ClockUnavailable)
                } else {
                    Ok(2001)
                }
            } else {
                Ok(1100)
            }
        }
    }
    let _case = CASES.lock().await;
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let bytes = encoded(ImageFormat::Png, 2, 3);
    register(&pool, "asset", &metadata(&bytes, ImageType::Png, 2, 3)).await;
    for (fail_at, unavailable) in [(1, false), (2, false), (3, false), (3, true)] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            fail_at,
            unavailable,
        };
        assert!(
            matches!(
                ingest_image(&pool, &token, &clock, request("asset", &bytes, "denied")).await,
                Err(ImageError::Staff(
                    StaffError::Denied | StaffError::ClockUnavailable
                ))
            ),
            "clock phase {fail_at}"
        );
        assert_eq!(counts(&pool).await, (0, 0));
    }
    ingest_image(
        &pool,
        &token,
        &FixedClock(1100),
        request("asset", &bytes, "first"),
    )
    .await
    .expect("stored");
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        fail_at: 3,
        unavailable: false,
    };
    assert!(matches!(
        ingest_image(&pool, &token, &clock, request("asset", &bytes, "first")).await,
        Err(ImageError::Staff(StaffError::Denied))
    ));
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        fail_at: 1,
        unavailable: true,
    };
    assert!(matches!(
        image_receipt(&pool, &token, &clock, "asset").await,
        Err(ImageError::Staff(StaffError::ClockUnavailable))
    ));
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        fail_at: 2,
        unavailable: false,
    };
    assert!(matches!(
        private_rendition(&pool, &token, &clock, "asset", 320).await,
        Err(ImageError::Staff(StaffError::Denied))
    ));
    assert_eq!(counts(&pool).await, (1, 1));
}
async fn wait_for_lock(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(800),async{loop{
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(pattern).fetch_one(pool).await.expect("lock probe");
        if waiting{break;}tokio::task::yield_now().await;
    }}).await.expect("image operation reached lock");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn storage_waits_bound_admitted_images_and_cancellation_recovers_capacity(pool: PgPool) {
    let _case = CASES.lock().await;
    seed(&pool).await;
    let bytes = encoded(ImageFormat::Png, 2, 3);
    let other_bytes = encoded(ImageFormat::Png, 3, 3);
    register(&pool, "first", &metadata(&bytes, ImageType::Png, 2, 3)).await;
    register(
        &pool,
        "second",
        &metadata(&other_bytes, ImageType::Png, 3, 3),
    )
    .await;
    let mut blocker = pool.begin().await.expect("storage blocker");
    blocker.execute("SELECT pg_advisory_xact_lock(hashtextextended('image-ingestion:slot-first',0)), pg_advisory_xact_lock(hashtextextended('image-ingestion:slot-second',0))").await.expect("pause both effects");
    let mut writers = Vec::new();
    for (asset, original, key) in [
        ("first", bytes.clone(), "slot-first"),
        ("second", other_bytes, "slot-second"),
    ] {
        let writer_pool = pool.clone();
        writers.push(tokio::spawn(async move {
            ingest_image(
                &writer_pool,
                &SessionToken::from_bytes([1; 32]),
                &FixedClock(1100),
                request(asset, &original, key),
            )
            .await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT pg_advisory_xact_lock%'").fetch_one(&pool).await.expect("both storage waits");
            if count == 2 { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("both images reached storage");
    assert!(matches!(
        ingest_image(
            &pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            request("first", &bytes, "third")
        )
        .await,
        Err(ImageError::Busy)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    let cancelled = writers.remove(0);
    cancelled.abort();
    assert!(cancelled.await.expect_err("cancelled").is_cancelled());
    ingest_image(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        request("first", &bytes, "recovered"),
    )
    .await
    .expect("capacity released while another image still waits");
    blocker.rollback().await.expect("resume second");
    writers
        .remove(0)
        .await
        .expect("second task")
        .expect("second image");
    assert_eq!(counts(&pool).await, (2, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_or_cancellation_during_image_claim_does_not_leave_private_orphans(pool: PgPool) {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    struct MovingClock(AtomicU64);
    impl Clock for MovingClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }
    let _case = CASES.lock().await;
    seed(&pool).await;
    let bytes = encoded(ImageFormat::Png, 2, 3);
    register(&pool, "asset", &metadata(&bytes, ImageType::Png, 2, 3)).await;
    for cancel in [false, true] {
        let mut blocker = pool.begin().await.expect("claim blocker");
        blocker.execute("SELECT asset_id FROM catalogue_media_serialization WHERE asset_id='asset' FOR UPDATE").await.expect("pause store");
        let current = Arc::new(MovingClock(AtomicU64::new(1100)));
        let writer_clock = current.clone();
        let writer_pool = pool.clone();
        let original = bytes.clone();
        let writer = tokio::spawn(async move {
            ingest_image(
                &writer_pool,
                &SessionToken::from_bytes([1; 32]),
                writer_clock.as_ref(),
                request("asset", &original, "paused"),
            )
            .await
        });
        wait_for_lock(
            &pool,
            "SELECT asset_id FROM catalogue_media_serialization%FOR UPDATE",
        )
        .await;
        if cancel {
            writer.abort();
            assert!(writer.await.expect_err("cancelled").is_cancelled());
            blocker.rollback().await.expect("unlock");
        } else {
            current.0.store(2001, Ordering::SeqCst);
            blocker.rollback().await.expect("resume after expiry");
            assert!(matches!(
                writer.await.expect("task"),
                Err(ImageError::Staff(StaffError::Denied))
            ));
        }
        assert_eq!(counts(&pool).await, (0, 0));
    }
    ingest_image(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        request("asset", &bytes, "recover"),
    )
    .await
    .expect("retry after cancellation");
    assert_eq!(counts(&pool).await, (1, 1));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_registration_and_bounded_dependency_failure_never_produce_image_proof(
    pool: PgPool,
) {
    let _case = CASES.lock().await;
    seed(&pool).await;
    let bytes = encoded(ImageFormat::Png, 2, 3);
    register(&pool, "asset", &metadata(&bytes, ImageType::Png, 2, 3)).await;
    let mut blocker = pool.begin().await.expect("effect blocker");
    blocker
        .execute("SELECT pg_advisory_xact_lock(hashtextextended('image-ingestion:paused',0))")
        .await
        .expect("pause after processing");
    let writer_pool = pool.clone();
    let original = bytes.clone();
    let writer = tokio::spawn(async move {
        ingest_image(
            &writer_pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            request("asset", &original, "paused"),
        )
        .await
    });
    wait_for_lock(&pool, "SELECT pg_advisory_xact_lock%").await;
    pool.execute("ALTER TABLE catalogue_media_assets DISABLE TRIGGER guard_media_asset_history; UPDATE catalogue_media_assets SET payload=jsonb_set(payload,'{image_type}','\"jpeg\"') WHERE asset_id='asset'").await.expect("hostile registration change");
    blocker.rollback().await.expect("resume");
    assert!(matches!(
        writer.await.expect("task"),
        Err(ImageError::MetadataMismatch)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    pool.execute("UPDATE catalogue_media_assets SET payload=jsonb_set(payload,'{image_type}','\"png\"') WHERE asset_id='asset'; ALTER TABLE catalogue_media_assets ENABLE TRIGGER guard_media_asset_history").await.expect("restore fixture");
    let mut blocker = pool.begin().await.expect("timeout blocker");
    blocker
        .execute("SELECT pg_advisory_xact_lock(hashtextextended('image-ingestion:timeout',0))")
        .await
        .expect("bounded wait");
    let error = ingest_image(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        request("asset", &bytes, "timeout"),
    )
    .await
    .expect_err("bounded dependency");
    match error {
        ImageError::Database(e) => assert_eq!(
            e.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("55P03")
        ),
        other => panic!("unexpected {other:?}"),
    }
    blocker.rollback().await.expect("release");
    assert_eq!(counts(&pool).await, (0, 0));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn immutable_hash_bound_complete_sets_and_hostile_stored_payloads_fail_closed(pool: PgPool) {
    let _case = CASES.lock().await;
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let bytes = encoded(ImageFormat::Png, 2, 3);
    register(&pool, "asset", &metadata(&bytes, ImageType::Png, 2, 3)).await;
    ingest_image(
        &pool,
        &token,
        &FixedClock(1100),
        request("asset", &bytes, "stored"),
    )
    .await
    .expect("stored");
    for invalid in [
        "UPDATE catalogue_image_originals SET reason='rewrite'",
        "DELETE FROM catalogue_image_originals",
        "UPDATE catalogue_image_renditions SET width_px=1",
        "DELETE FROM catalogue_image_renditions",
        "INSERT INTO catalogue_image_renditions SELECT asset_id,recipe,640,width_px,height_px,image_type,sha256,bytes FROM catalogue_image_renditions WHERE asset_id='asset'",
    ] {
        assert!(pool.execute(invalid).await.is_err(), "accepted {invalid}");
    }
    let other = encoded(ImageFormat::Jpeg, 3, 3);
    register(&pool, "partial", &metadata(&other, ImageType::Jpeg, 3, 3)).await;
    let error=sqlx::query("INSERT INTO catalogue_image_originals(asset_id,effect_key,sha256,bytes,image_type,source_width_px,source_height_px,display_width_px,display_height_px,actor,reason) VALUES('partial','partial',$1,$2,'jpeg',3,3,3,3,'fixture-catalogue','hostile incomplete set')")
        .bind(Sha256::digest(&other).to_vec()).bind(&other).execute(&pool).await.expect_err("partial image cannot commit");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    assert_eq!(counts(&pool).await, (1, 1));
    pool.execute("ALTER TABLE catalogue_image_renditions DISABLE TRIGGER guard_image_renditions")
        .await
        .expect("hostile import fixture");
    sqlx::query(
        "UPDATE catalogue_image_renditions SET bytes=$1,sha256=sha256($1) WHERE asset_id='asset'",
    )
    .bind(b"not an image".as_slice())
    .execute(&pool)
    .await
    .expect("corrupted but hash consistent bytes");
    assert!(matches!(
        private_rendition(&pool, &token, &FixedClock(1100), "asset", 320).await,
        Err(ImageError::InvalidStoredData)
    ));
    pool.execute("DELETE FROM catalogue_image_renditions WHERE asset_id='asset'")
        .await
        .expect("missing rendition fixture");
    assert!(matches!(
        image_receipt(&pool, &token, &FixedClock(1100), "asset").await,
        Err(ImageError::InvalidStoredData)
    ));
}
#[sqlx::test(migrations = false)]
async fn schema_upgrade_preserves_rights_and_starts_without_verified_images(pool: PgPool) {
    let _case = CASES.lock().await;
    MIGRATOR.run_to(10, &pool).await.expect("prior schema");
    seed(&pool).await;
    let bytes = encoded(ImageFormat::Png, 2, 3);
    register(&pool, "asset", &metadata(&bytes, ImageType::Png, 2, 3)).await;
    let permission = commerce_domain::media::MediaRightsChange::Grant {
        use_scope: commerce_domain::media::MediaUse::Inspiration,
        valid_from: 1100,
        term: commerce_domain::media::RightsTerm::Perpetual,
        permission_ref: "prior isolated image permission".into(),
    };
    commerce_infrastructure::media::record_media_rights(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        commerce_infrastructure::media::MediaRightsRequest {
            asset_id: "asset",
            expected_revision: 0,
            effect_key: "prior-rights",
            change: &permission,
            reason: "prior image schema test",
            confirmed: true,
        },
    )
    .await
    .expect("prior rights");
    MIGRATOR.run(&pool).await.expect("image schema expansion");
    let old = commerce_infrastructure::media::inspect_media(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        "asset",
    )
    .await
    .expect("old reader")
    .expect("old media");
    assert_eq!(old.rights_revision, Some(1));
    assert!(!old.product_rights_active);
    assert_eq!(counts(&pool).await, (0, 0));
    let root: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalogue_media_assets WHERE asset_id='asset'")
            .fetch_one(&pool)
            .await
            .expect("registry oracle");
    assert_eq!(root, 1);
    ingest_image(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        request("asset", &bytes, "upgraded"),
    )
    .await
    .expect("actual ingestion after upgrade");
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn incompressible_alpha_derivatives_exceeding_budget_are_rejected_atomically(pool: PgPool) {
    let _case = CASES.lock().await;
    seed(&pool).await;
    let mut image = image::RgbaImage::new(960, 960);
    let mut state = 0x1357_2468_u32;
    for pixel in image.pixels_mut() {
        let mut channels = [0; 4];
        for channel in &mut channels {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *channel = state.to_be_bytes()[0];
        }
        *pixel = image::Rgba(channels);
    }
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(image)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("synthetic noise fixture");
    register(&pool, "budget", &metadata(&bytes, ImageType::Png, 960, 960)).await;
    assert!(matches!(
        ingest_image(
            &pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            request("budget", &bytes, "budget")
        )
        .await,
        Err(ImageError::ResourceLimit)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
}
