static SEEDS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
// Retain this guard for the whole case: byte inspection shares the decoder budget.
async fn media_seed(pool: &PgPool) -> tokio::sync::MutexGuard<'static, ()> {
    let case = SEEDS.lock().await;
    seed(pool).await;
    let mut bytes = Vec::new();
    image::DynamicImage::new_rgb8(32, 24)
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
        .expect("synthetic bytes");
    let metadata = MediaAssetDraft {
        sha256: Sha256::digest(&bytes).into(),
        image_type: ImageType::Jpeg,
        width_px: 32,
        height_px: 24,
        kind: MediaKind::OriginalPhoto,
        fixture: false,
        source_ref: "isolated synthetic candidate".into(),
    };
    register_media(
        pool,
        &author(),
        &FixedClock(1100),
        RegisterMediaRequest {
            asset_id: "photo",
            effect_key: "photo",
            draft: &metadata,
            reason: "synthetic registry",
            confirmed: true,
        },
    )
    .await
    .expect("register");
    ingest_image(
        pool,
        &author(),
        &FixedClock(1100),
        IngestImageRequest {
            asset_id: "photo",
            effect_key: "ingest",
            bytes: &bytes,
            reason: "synthetic ingestion",
            confirmed: true,
        },
    )
    .await
    .expect("ingest");
    rights(
        pool,
        0,
        &MediaRightsChange::Grant {
            use_scope: MediaUse::ProductPhotography,
            valid_from: 1100,
            term: RightsTerm::Until { expires_at: 2000 },
            permission_ref: "synthetic permission".into(),
        },
    )
    .await;
    case
}
async fn rights(pool: &PgPool, expected: i32, change: &MediaRightsChange) {
    record_media_rights(
        pool,
        &author(),
        &FixedClock(1100),
        MediaRightsRequest {
            asset_id: "photo",
            expected_revision: expected,
            effect_key: &format!("rights-{expected}"),
            change,
            reason: "synthetic rights",
            confirmed: true,
        },
    )
    .await
    .expect("rights");
}
