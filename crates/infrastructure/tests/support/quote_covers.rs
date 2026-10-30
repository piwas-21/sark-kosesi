// Complete synthetic candidates used to exercise the actual public cursor boundary.
async fn covers_candidate(pool: &PgPool, sku: &str) {
    definition(
        pool,
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
        &format!("variant-{sku}"),
    )
    .await;
    let mut spec = draft(false);
    spec.components[0].component_code = "SEAT-COVER".into();
    record_specification(
        pool,
        &author(),
        &FixedClock(1100),
        SpecificationRequest {
            sku,
            bom_revision: 1,
            expected_revision: 0,
            effect_key: &format!("spec-{sku}"),
            draft: &spec,
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("spec");
    record_price_revision(
        pool,
        &author(),
        &FixedClock(1100),
        PriceRevisionRequest {
            sku,
            expected_revision: 0,
            effect_key: &format!("price-{sku}"),
            price: commerce_domain::Money::eur(12345).expect("synthetic money"),
            evidence_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("price");
    record_factual_review(
        pool,
        &reviewer(),
        &FixedClock(1100),
        FactReviewRequest {
            sku,
            target: selected().facts,
            expected_review_revision: 0,
            effect_key: &format!("facts-{sku}"),
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
        pool,
        &reviewer(),
        &FixedClock(1100),
        ImageReviewRequest {
            sku,
            target: &selected().image,
            expected_revision: 0,
            effect_key: &format!("image-{sku}"),
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
        pool,
        &author(),
        &FixedClock(1100),
        TranslationRequest {
            sku,
            locale: Locale::Nl,
            specification_revision: 1,
            source_revision: None,
            expected_revision: 0,
            effect_key: &format!("copy-{sku}"),
            draft: &copy(),
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("copy");
    record_review(
        pool,
        &reviewer(),
        &FixedClock(1100),
        TranslationReviewRequest {
            sku,
            locale: Locale::Nl,
            translation_revision: 1,
            expected_review_revision: 0,
            effect_key: &format!("copy-review-{sku}"),
            decision: TranslationDecision::Approved,
            evidence_ref: "synthetic",
            qualification_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("copy review");
    record_publication(
        pool,
        &author(),
        &FixedClock(1100),
        PublicationRequest {
            sku,
            locale: Locale::Nl,
            expected_revision: 0,
            effect_key: &format!("publish-{sku}"),
            choice: &choice(),
            evidence_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("publication");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn covers_only_quote_needs_no_foam_and_never_creates_a_foam_pool(pool: PgPool) {
    let _case = media_seed(&pool).await;
    covers_candidate(&pool, "SET-COVERS").await;
    stock(&pool, 2, 0, 0, 0).await;
    pool.execute("DELETE FROM stock_balances WHERE component_code='SEAT-FOAM'")
        .await
        .expect("no foam received");
    let mut p = policy();
    p.products[0].sku = "SET-COVERS".into();
    let mut r = basket(1);
    r.lines[0].sku = "SET-COVERS".into();
    let q = quote(&pool, &r, &[p]).await.expect("covers available");
    assert_eq!(q.totals.total.gross.minor_units(), 13555);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM stock_balances WHERE component_code='SEAT-FOAM'")
            .fetch_one(&pool)
            .await
            .expect("no fictional pool");
    assert_eq!(count, 0);
}
