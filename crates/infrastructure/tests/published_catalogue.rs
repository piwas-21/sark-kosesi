#[allow(dead_code, unused_imports)] // Shared isolated workflow fixture.
mod fixture {
    include!("../../../tests/fixtures/approved_catalogue.rs");
}
use commerce_infrastructure::{image_ingestion::ImageError, published_catalogue::*};
use fixture::*;
async fn activate(pool: &PgPool) {
    approve(pool).await;
    record_publication(
        pool,
        &author(),
        &FixedClock(1100),
        request("activate", &choice()),
    )
    .await
    .expect("activate");
}
async fn public(pool: &PgPool) -> Option<PublishedProduct> {
    published_product(pool, &FixedClock(1100), Locale::Nl, "SET-FILLED")
        .await
        .expect("projection")
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn only_current_approved_complete_customer_facts_cross_projection(pool: PgPool) {
    let _case = media_seed(&pool).await;
    assert!(public(&pool).await.is_none());
    assert!(
        published_page(&pool, &FixedClock(1100), Locale::Nl, None)
            .await
            .expect("empty")
            .items
            .is_empty()
    );
    activate(&pool).await;
    let product = public(&pool).await.expect("published");
    assert_eq!(
        (
            &*product.sku,
            &*product.family_code,
            &*product.family_slug,
            &*product.fill
        ),
        ("SET-FILLED", "ROOM", "isolated-room", "with-foam")
    );
    assert_eq!(
        (product.price.minor_units(), product.publication_revision),
        (12345, 1)
    );
    assert_eq!(product.copy, copy());
    assert_eq!(product.components.len(), 2);
    assert_eq!(product.components[0].quantity, 2);
    assert_eq!(product.footprint.width_mm, 1000);
    assert_eq!(product.parcels[0].gross_weight_g, 3000);
    assert_eq!(product.image.renditions.len(), 1);
    assert!(!product.in_stock);
    assert_eq!(
        published_page(&pool, &FixedClock(1100), Locale::Nl, None)
            .await
            .expect("page")
            .items,
        vec![product]
    );
    for locale in [
        Locale::De,
        Locale::Fr,
        Locale::En,
        Locale::Ar,
        Locale::Tr,
        Locale::Kmr,
    ] {
        assert!(
            published_product(&pool, &FixedClock(1100), locale, "SET-FILLED")
                .await
                .expect("no fallback")
                .is_none()
        );
    }
    assert!(matches!(
        published_product(
            &pool,
            &FixedClock(1100),
            Locale::Nl,
            "DEMO-COURTYARD-COVERS"
        )
        .await,
        Err(PublishedError::InvalidRequest)
    ));
    assert!(
        published_page(&pool, &FixedClock(1100), Locale::Nl, Some("SET-FILLED"))
            .await
            .expect("cursor")
            .items
            .is_empty()
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn read_through_stock_uses_every_physical_component_and_never_serializes_balances(
    pool: PgPool,
) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    for (cover, foam, quarantine, reserved, expected) in [
        (1, 50, 0, 0, false),
        (2, 1, 0, 0, false),
        (4, 4, 0, 0, true),
        (4, 4, 3, 0, false),
        (4, 4, 1, 2, false),
        (4, 4, 1, 1, true),
        (0, 0, 0, 0, false),
    ] {
        sqlx::query("INSERT INTO stock_balances(component_code,on_hand,quarantine,reserved) VALUES('SEAT-COVER',$1,$3,$4),('SEAT-FOAM',$2,0,0) ON CONFLICT(component_code) DO UPDATE SET on_hand=EXCLUDED.on_hand,quarantine=EXCLUDED.quarantine,reserved=EXCLUDED.reserved").bind(cover).bind(foam).bind(quarantine).bind(reserved).execute(&pool).await.expect("isolated ledger snapshot");
        assert_eq!(
            public(&pool).await.expect("product").in_stock,
            expected,
            "{cover}/{foam}/{quarantine}/{reserved}"
        );
    }
    sqlx::query("DELETE FROM stock_balances WHERE component_code='SEAT-FOAM'")
        .execute(&pool)
        .await
        .expect("unreceived");
    assert!(!public(&pool).await.expect("product").in_stock);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn withdrawal_replay_and_stale_revisions_remove_product_and_every_derivative(pool: PgPool) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    let image = published_image(&pool, &FixedClock(1100), Locale::Nl, "SET-FILLED", 1, 320)
        .await
        .expect("image")
        .expect("bytes");
    assert_eq!(
        (image.width, image.height, image.image_type.as_str()),
        (32, 24, "jpeg")
    );
    assert!(!image.bytes.is_empty());
    for (sku, rev, edge) in [
        ("MISSING", 1, 320),
        ("SET-FILLED", 2, 320),
        ("SET-FILLED", 1, 640),
    ] {
        assert!(
            published_image(&pool, &FixedClock(1100), Locale::Nl, sku, rev, edge)
                .await
                .expect("absent")
                .is_none()
        );
    }
    let mut r = request("withdraw", &PublicationChoice::Withdraw {});
    r.expected_revision = 1;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("withdraw");
    assert!(public(&pool).await.is_none());
    assert!(
        published_image(&pool, &FixedClock(1100), Locale::Nl, "SET-FILLED", 1, 320)
            .await
            .expect("withdrawn bytes")
            .is_none()
    );
    assert!(
        record_publication(
            &pool,
            &author(),
            &FixedClock(1100),
            request("activate", &choice())
        )
        .await
        .expect("replay")
        .replayed
    );
    assert!(public(&pool).await.is_none());
    let activation = choice();
    let mut r = request("republish", &activation);
    r.expected_revision = 2;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("republish");
    assert_eq!(
        public(&pool).await.expect("product").publication_revision,
        3
    );
    assert!(
        published_image(&pool, &FixedClock(1100), Locale::Nl, "SET-FILLED", 1, 320)
            .await
            .expect("old URL")
            .is_none()
    );
    assert!(
        published_image(&pool, &FixedClock(1100), Locale::Nl, "SET-FILLED", 3, 320)
            .await
            .expect("new URL")
            .is_some()
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn current_source_edits_and_rejections_never_silently_reactivate(pool: PgPool) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    copy_review(
        &pool,
        Locale::Nl,
        1,
        1,
        TranslationDecision::Rejected,
        "reject-copy",
    )
    .await;
    assert!(public(&pool).await.is_none());
    copy_review(
        &pool,
        Locale::Nl,
        1,
        2,
        TranslationDecision::Approved,
        "reapprove-copy",
    )
    .await;
    assert!(public(&pool).await.is_none());
    let mut selection = selected();
    selection.translation_review_revision = 3;
    let c = PublicationChoice::Activate { selection };
    let mut r = request("republish", &c);
    r.expected_revision = 1;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("republish");
    assert!(public(&pool).await.is_some());
    price(&pool, 1, "new-price").await;
    assert!(public(&pool).await.is_none());
    assert!(
        published_image(&pool, &FixedClock(1100), Locale::Nl, "SET-FILLED", 2, 320)
            .await
            .expect("stale image")
            .is_none()
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn rights_expiry_revocation_and_clock_failures_fail_closed_at_final_read(pool: PgPool) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    for clock in [FixedClock(1099), FixedClock(2000), FixedClock(3000)] {
        assert!(
            published_product(&pool, &clock, Locale::Nl, "SET-FILLED")
                .await
                .expect("expired")
                .is_none()
        );
    }
    assert!(
        published_product(&pool, &FixedClock(1999), Locale::Nl, "SET-FILLED")
            .await
            .expect("active")
            .is_some()
    );
    let expire = StepClock {
        calls: AtomicUsize::new(0),
        after: 1,
        value: Ok(2000),
    };
    assert!(
        published_product(&pool, &expire, Locale::Nl, "SET-FILLED")
            .await
            .expect("final expiry")
            .is_none()
    );
    for after in [0, 1] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            after,
            value: Err(ContextError::ClockUnavailable),
        };
        assert!(matches!(
            published_product(&pool, &clock, Locale::Nl, "SET-FILLED").await,
            Err(PublishedError::ClockUnavailable)
        ));
    }
    // A complete first projection has two clock samples; the second projection
    // occurs only after the sanitized derivative has been decoded and verified.
    let expire = StepClock {
        calls: AtomicUsize::new(0),
        after: 2,
        value: Ok(2000),
    };
    assert!(
        published_image(&pool, &expire, Locale::Nl, "SET-FILLED", 1, 320)
            .await
            .expect("post CPU expiry")
            .is_none()
    );
    let failure = StepClock {
        calls: AtomicUsize::new(0),
        after: 2,
        value: Err(ContextError::ClockUnavailable),
    };
    assert!(matches!(
        published_image(&pool, &failure, Locale::Nl, "SET-FILLED", 1, 320).await,
        Err(PublishedError::ClockUnavailable)
    ));
    rights(&pool, 1, &MediaRightsChange::Revoke).await;
    assert!(public(&pool).await.is_none());
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn malformed_requests_and_closed_dependencies_never_return_private_or_partial_data(
    pool: PgPool,
) {
    for sku in ["", "bad/path", "bad space", "x<script>"] {
        assert!(matches!(
            published_product(&pool, &FixedClock(1100), Locale::Nl, sku).await,
            Err(PublishedError::InvalidRequest)
        ));
        assert!(matches!(
            published_page(&pool, &FixedClock(1100), Locale::Nl, Some(sku)).await,
            Err(PublishedError::InvalidRequest)
        ));
    }
    for (rev, edge) in [(0, 320), (-1, 320), (1, 0), (1, 2000)] {
        assert!(matches!(
            published_image(
                &pool,
                &FixedClock(1100),
                Locale::Nl,
                "SET-FILLED",
                rev,
                edge
            )
            .await,
            Err(PublishedError::InvalidRequest)
        ));
    }
    pool.close().await;
    assert!(matches!(
        public_after_close(&pool).await,
        Err(PublishedError::Database(_))
    ));
}
async fn public_after_close(pool: &PgPool) -> Result<Option<PublishedProduct>, PublishedError> {
    published_product(pool, &FixedClock(1100), Locale::Nl, "SET-FILLED").await
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn pagination_progresses_through_withdrawn_candidates_without_truncation(pool: PgPool) {
    let _case = media_seed(&pool).await;
    for number in 0..27 {
        let sku = format!("PAGE-{number:02}");
        definition(
            &pool,
            &CatalogueCommand::Variant {
                sku: sku.clone(),
                family_code: "ROOM".into(),
                size_code: "isolated".into(),
                textile_code: "isolated".into(),
                fill: Fill::WithFoam,
                lines: lines(2),
            },
            &format!("variant-{number}"),
        )
        .await;
        let r = PublicationRequest {
            sku: &sku,
            locale: Locale::Nl,
            expected_revision: 0,
            effect_key: &format!("withdraw-{number}"),
            choice: &PublicationChoice::Withdraw {},
            evidence_ref: "isolated withdrawal",
            reason: "synthetic pagination",
            confirmed: true,
        };
        record_publication(&pool, &author(), &FixedClock(1100), r)
            .await
            .expect("withdraw");
    }
    let page = published_page(&pool, &FixedClock(1100), Locale::Nl, None)
        .await
        .expect("page");
    assert!(page.items.is_empty());
    assert_eq!(page.next_after.as_deref(), Some("PAGE-24"));
    let page = published_page(
        &pool,
        &FixedClock(1100),
        Locale::Nl,
        page.next_after.as_deref(),
    )
    .await
    .expect("next");
    assert!(page.items.is_empty());
    assert!(page.next_after.is_none());
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn covers_only_uses_shared_cover_without_requiring_or_creating_foam_stock(pool: PgPool) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    let sku = "SET-COVERS";
    definition(
        &pool,
        &CatalogueCommand::Variant {
            sku: sku.into(),
            family_code: "ROOM".into(),
            size_code: "isolated size".into(),
            textile_code: "isolated textile".into(),
            fill: Fill::CoversOnly,
            lines: vec![BomLineDraft {
                component_code: "SEAT-COVER".into(),
                quantity: 2,
            }],
        },
        "covers-variant",
    )
    .await;
    let mut spec = draft(false);
    spec.components[0].component_code = "SEAT-COVER".into();
    record_specification(
        &pool,
        &author(),
        &FixedClock(1100),
        SpecificationRequest {
            sku,
            bom_revision: 1,
            expected_revision: 0,
            effect_key: "covers-spec",
            draft: &spec,
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("spec");
    record_price_revision(
        &pool,
        &author(),
        &FixedClock(1100),
        PriceRevisionRequest {
            sku,
            expected_revision: 0,
            effect_key: "covers-price",
            price: commerce_domain::Money::eur(9007199254740991).expect("safe integer"),
            evidence_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("price");
    record_factual_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        FactReviewRequest {
            sku,
            target: selected().facts,
            expected_review_revision: 0,
            effect_key: "covers-facts",
            decision: FactDecision::Approved,
            evidence_ref: "synthetic",
            qualification_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("facts");
    record_image_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        ImageReviewRequest {
            sku,
            target: &selected().image,
            expected_revision: 0,
            effect_key: "covers-image",
            decision: ImageDecision::Approved,
            evidence_ref: "synthetic",
            qualification_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("image");
    record_translation(
        &pool,
        &author(),
        &FixedClock(1100),
        TranslationRequest {
            sku,
            locale: Locale::Nl,
            specification_revision: 1,
            source_revision: None,
            expected_revision: 0,
            effect_key: "covers-copy",
            draft: &copy(),
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("copy");
    record_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        TranslationReviewRequest {
            sku,
            locale: Locale::Nl,
            translation_revision: 1,
            expected_review_revision: 0,
            effect_key: "covers-copy-review",
            decision: TranslationDecision::Approved,
            evidence_ref: "synthetic",
            qualification_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("copy review");
    let r = PublicationRequest {
        sku,
        locale: Locale::Nl,
        expected_revision: 0,
        effect_key: "covers-publish",
        choice: &choice(),
        evidence_ref: "synthetic",
        reason: "synthetic",
        confirmed: true,
    };
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("activate covers");
    sqlx::query("INSERT INTO stock_balances(component_code,on_hand) VALUES('SEAT-COVER',2)")
        .execute(&pool)
        .await
        .expect("physical receipt");
    let covers = published_product(&pool, &FixedClock(1100), Locale::Nl, sku)
        .await
        .expect("projection")
        .expect("covers");
    assert!(covers.in_stock);
    assert_eq!(covers.components.len(), 1);
    assert_eq!(covers.fill, "covers-only");
    assert_eq!(covers.price.minor_units(), 9007199254740991);
    assert!(!public(&pool).await.expect("filled").in_stock);
    sqlx::query("UPDATE stock_balances SET reserved=1 WHERE component_code='SEAT-COVER'")
        .execute(&pool)
        .await
        .expect("shared reservation");
    assert!(
        !published_product(&pool, &FixedClock(1100), Locale::Nl, sku)
            .await
            .expect("projection")
            .expect("covers")
            .in_stock
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM stock_balances")
            .fetch_one(&pool)
            .await
            .expect("physical pools"),
        1
    );
}
async fn waiting_on(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(800),async {loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(pattern).fetch_one(pool).await.expect("wait oracle");
        if waiting {break;}tokio::task::yield_now().await;
    }}).await.expect("actual database wait");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn waiting_public_reader_sees_committed_withdrawal_and_cancellation_releases_claims(
    pool: PgPool,
) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    let mut withdrawal = pool.begin().await.expect("withdrawal");
    withdrawal
        .execute("UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku='SET-FILLED'")
        .await
        .expect("claim");
    withdrawal.execute("INSERT INTO catalogue_publication_revisions(sku,locale,revision,effect_key,choice,evidence_ref,actor,reason,evaluated_at) VALUES('SET-FILLED','nl',2,'pending-withdraw','{\"action\":\"withdraw\"}','synthetic','author','synthetic',1100)").await.expect("pending withdrawal");
    let owned = pool.clone();
    let reader = tokio::spawn(async move { public_after_close(&owned).await });
    waiting_on(&pool, "SELECT sku FROM catalogue_variants%").await;
    withdrawal.commit().await.expect("withdraw");
    assert!(reader.await.expect("reader").expect("projection").is_none());
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute("UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku='SET-FILLED'")
        .await
        .expect("claim");
    let owned = pool.clone();
    let reader = tokio::spawn(async move { public_after_close(&owned).await });
    waiting_on(&pool, "SELECT sku FROM catalogue_variants%").await;
    reader.abort();
    assert!(reader.await.expect_err("cancel").is_cancelled());
    blocker.rollback().await.expect("resume");
    assert!(public(&pool).await.is_none());
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn query_waits_complete_before_final_product_rights_evaluation(pool: PgPool) {
    struct MovingClock(std::sync::atomic::AtomicU64);
    impl Clock for MovingClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    pool.execute("ALTER FUNCTION publication_sources_current(text,text,jsonb,bigint) RENAME TO original_publication_sources_current; CREATE FUNCTION publication_sources_current(item text,language text,choice jsonb,now_seconds bigint) RETURNS boolean LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(hashtextextended('published-late-read',0)); RETURN original_publication_sources_current(item,language,choice,now_seconds); END; $$").await.expect("real query wait instrument");
    let fresh = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .expect("fresh cache");
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute("SELECT pg_advisory_xact_lock(hashtextextended('published-late-read',0))")
        .await
        .expect("claim");
    let clock = std::sync::Arc::new(MovingClock(std::sync::atomic::AtomicU64::new(1100)));
    let owned_clock = clock.clone();
    let owned = fresh.clone();
    let reader = tokio::spawn(async move {
        published_product(&owned, owned_clock.as_ref(), Locale::Nl, "SET-FILLED").await
    });
    waiting_on(&pool, "SELECT publication_sources_current%").await;
    clock.0.store(2000, Ordering::SeqCst);
    blocker.rollback().await.expect("resume");
    assert!(reader.await.expect("reader").expect("expired").is_none());
    fresh.close().await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn corrupt_route_claim_source_and_rendition_cannot_escape_customer_boundary(pool: PgPool) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    pool.execute("ALTER TABLE catalogue_publication_revisions DISABLE TRIGGER USER")
        .await
        .expect("isolated corruption setup");
    sqlx::query("UPDATE catalogue_publication_revisions SET choice=$1::jsonb")
        .bind(serde_json::to_string(&choice()).expect("JSON"))
        .execute(&pool)
        .await
        .expect("unchanged");
    pool.execute("UPDATE catalogue_publication_revisions SET choice=jsonb_set(choice,'{selection,family_slug}','\"wrong-room\"')").await.expect("corrupt path");
    assert!(matches!(
        public_after_close(&pool).await,
        Err(PublishedError::InvalidStoredData)
    ));
    pool.execute("UPDATE catalogue_publication_revisions SET choice=jsonb_set(choice,'{selection,family_slug}','\"isolated-room\"'); UPDATE catalogue_publication_revisions SET choice=jsonb_set(choice,'{selection,facts,price_revision}','99')").await.expect("missing source");
    assert!(public(&pool).await.is_none());
    sqlx::query("UPDATE catalogue_publication_revisions SET choice=$1::jsonb")
        .bind(serde_json::to_string(&choice()).expect("JSON"))
        .execute(&pool)
        .await
        .expect("restore");
    pool.execute("ALTER TABLE catalogue_image_renditions DISABLE TRIGGER USER; UPDATE catalogue_image_renditions SET width_px=1").await.expect("corrupt derivative metadata");
    assert!(matches!(
        published_image(&pool, &FixedClock(1100), Locale::Nl, "SET-FILLED", 1, 320).await,
        Err(PublishedError::Publication(PublicationError::Image(
            commerce_infrastructure::image_review::ImageReviewError::Image(
                ImageError::InvalidStoredData
            )
        )))
    ));
    pool.execute("UPDATE catalogue_image_renditions SET width_px=32; ALTER TABLE catalogue_media_assets DROP CONSTRAINT media_serialization_required; DELETE FROM catalogue_media_serialization WHERE asset_id='photo'").await.expect("missing serialization claim");
    assert!(matches!(
        public_after_close(&pool).await,
        Err(PublishedError::InvalidStoredData)
    ));
}
include!("support/published_candidate.rs");
#[sqlx::test(migrator = "MIGRATOR")]
async fn cursor_comparison_and_order_match_ascii_under_conflicting_database_collation(
    pool: PgPool,
) {
    let _case = media_seed(&pool).await;
    pool.execute("CREATE COLLATION cursor_locale(provider=icu,locale='en-US-u-ka-shifted'); ALTER TABLE catalogue_publication_revisions ALTER COLUMN sku TYPE text COLLATE cursor_locale").await.expect("isolated conflicting collation");
    candidate(&pool, "Z-2").await;
    candidate(&pool, "Z1").await;
    for sku in (0..24).map(|n| format!("B-{n:02}")).chain(["Z3".into()]) {
        definition(
            &pool,
            &CatalogueCommand::Variant {
                sku: sku.clone(),
                family_code: "ROOM".into(),
                size_code: "isolated".into(),
                textile_code: "isolated".into(),
                fill: Fill::WithFoam,
                lines: lines(2),
            },
            &format!("variant-{sku}"),
        )
        .await;
        record_publication(
            &pool,
            &author(),
            &FixedClock(1100),
            PublicationRequest {
                sku: &sku,
                locale: Locale::Nl,
                expected_revision: 0,
                effect_key: &format!("withdraw-{sku}"),
                choice: &PublicationChoice::Withdraw {},
                evidence_ref: "synthetic",
                reason: "synthetic",
                confirmed: true,
            },
        )
        .await
        .expect("withdrawn candidate");
    }
    let native: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT sku FROM catalogue_publication_revisions WHERE sku LIKE 'Z%' ORDER BY sku",
    )
    .fetch_all(&pool)
    .await
    .expect("independent database locale oracle");
    assert_eq!(native, vec!["Z1", "Z-2", "Z3"]);
    let first = published_page(&pool, &FixedClock(1100), Locale::Nl, None)
        .await
        .expect("first");
    assert_eq!(first.next_after.as_deref(), Some("Z-2"));
    assert_eq!(
        first
            .items
            .iter()
            .map(|p| p.sku.as_str())
            .collect::<Vec<_>>(),
        vec!["Z-2"]
    );
    let next = published_page(
        &pool,
        &FixedClock(1100),
        Locale::Nl,
        first.next_after.as_deref(),
    )
    .await
    .expect("next");
    assert!(next.next_after.is_none());
    assert_eq!(
        next.items
            .iter()
            .map(|p| p.sku.as_str())
            .collect::<Vec<_>>(),
        vec!["Z1"]
    );
}
