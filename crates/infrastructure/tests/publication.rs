#[allow(dead_code, unused_imports)] // Shared isolated workflow fixture.
mod fixture {
    include!("../../../tests/fixtures/approved_catalogue.rs");
}
use fixture::*;
async fn counts(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM catalogue_publication_revisions),(SELECT count(*) FROM catalogue_family_routes)").fetch_one(pool).await.expect("counts")
}
async fn status(pool: &PgPool, locale: Locale) -> PublicationStatus {
    publication_status(pool, &author(), &FixedClock(1100), "SET-FILLED", locale)
        .await
        .expect("private status")
        .expect("history")
}
async fn raw(
    pool: &PgPool,
    value: &serde_json::Value,
    revision: i32,
    key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO catalogue_publication_revisions(sku,locale,revision,effect_key,choice,evidence_ref,actor,reason,evaluated_at) VALUES('SET-FILLED','nl',$1,$2,$3::jsonb,'isolated import','author','isolated reason',1100)").bind(revision).bind(key).bind(value.to_string()).execute(pool).await.map(|_|())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn explicit_complete_approved_selection_activates_without_changing_fixture_projection(
    pool: PgPool,
) {
    let _case = media_seed(&pool).await;
    assert!(
        publication_status(
            &pool,
            &author(),
            &FixedClock(1100),
            "SET-FILLED",
            Locale::Nl
        )
        .await
        .expect("none")
        .is_none()
    );
    assert!(matches!(
        record_publication(
            &pool,
            &author(),
            &FixedClock(1100),
            request("missing-approvals", &choice())
        )
        .await,
        Err(PublicationError::NotEligible)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    approve(&pool).await;
    let receipt = record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("activate", &choice()),
    )
    .await
    .expect("activate");
    assert_eq!((receipt.revision, receipt.replayed), (1, false));
    let state = status(&pool, Locale::Nl).await;
    assert!(state.eligible);
    assert_eq!(state.record.choice, choice());
    assert_eq!(state.record.actor, "author");
    assert_eq!(counts(&pool).await, (1, 1));
    let route: (String, String) =
        sqlx::query_as("SELECT family_code,slug FROM catalogue_family_routes WHERE locale='nl'")
            .fetch_one(&pool)
            .await
            .expect("route");
    assert_eq!(route, ("ROOM".into(), "isolated-room".into()));
    let items = commerce_infrastructure::catalogue(&pool, Locale::Nl)
        .await
        .expect("legacy preview");
    assert!(
        items
            .iter()
            .all(|item| item.fixture && item.sku.starts_with("DEMO-"))
    );
    let fields: (Option<i64>, Option<String>) =
        sqlx::query_as("SELECT minor_units,slug FROM catalogue_items WHERE sku='SET-FILLED'")
            .fetch_one(&pool)
            .await
            .expect("legacy fields");
    assert_eq!(fields, (None, None));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn withdrawal_and_historical_replay_never_restore_activation(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("activate", &choice()),
    )
    .await
    .expect("activate");
    let mut r = request("withdraw", &PublicationChoice::Withdraw {});
    r.expected_revision = 1;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("withdraw");
    assert!(!status(&pool, Locale::Nl).await.eligible);
    assert!(
        record_publication(
            &pool,
            &author(),
            &FixedClock(1100),
            request("activate", &choice())
        )
        .await
        .expect("old receipt")
        .replayed
    );
    assert_eq!(status(&pool, Locale::Nl).await.record.revision, 2);
    assert!(!status(&pool, Locale::Nl).await.eligible);
    assert_eq!(counts(&pool).await, (2, 1));
}

#[test]
fn closed_selection_cannot_claim_authority_or_unknown_source_fields() {
    let mut value = serde_json::to_value(choice()).expect("choice");
    value["actor"] = serde_json::json!("owner");
    assert!(serde_json::from_value::<PublicationChoice>(value).is_err());
    let mut value = serde_json::to_value(choice()).expect("choice");
    value["selection"]["image"]["published"] = serde_json::json!(true);
    assert!(serde_json::from_value::<PublicationChoice>(value).is_err());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn catalogue_authority_confirmation_and_bounded_closed_requests_are_required(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    let c = choice();
    for token in [
        reviewer(),
        SessionToken::from_bytes([4; 32]),
        SessionToken::from_bytes([99; 32]),
    ] {
        assert!(matches!(
            record_publication(&pool, &token, &FixedClock(1100), request("denied", &c)).await,
            Err(PublicationError::Staff(StaffError::Denied))
        ));
        assert!(matches!(
            publication_status(&pool, &token, &FixedClock(1100), "SET-FILLED", Locale::Nl).await,
            Err(PublicationError::Staff(StaffError::Denied))
        ));
    }
    for field in 0..8 {
        let mut r = request("invalid", &c);
        match field {
            0 => r.confirmed = false,
            1 => r.sku = "DEMO-FILLED",
            2 => r.expected_revision = -1,
            3 => r.expected_revision = i32::MAX,
            4 => r.effect_key = "bad/key",
            5 => r.evidence_ref = " ",
            6 => r.reason = " ",
            _ => r.locale = Locale::De,
        };
        assert!(
            matches!(
                record_publication(&pool, &author(), &FixedClock(1100), r).await,
                Err(PublicationError::InvalidRequest)
            ),
            "field {field}"
        );
    }
    let mut invalid = selected();
    invalid.factual_review_revision = 0;
    let c = PublicationChoice::Activate { selection: invalid };
    assert!(matches!(
        record_publication(
            &pool,
            &author(),
            &FixedClock(1100),
            request("invalid-selection", &c)
        )
        .await,
        Err(PublicationError::InvalidRequest)
    ));
    assert!(matches!(
        publication_status(&pool, &author(), &FixedClock(1100), "bad/sku", Locale::Nl).await,
        Err(PublicationError::InvalidRequest)
    ));
    let c = choice();
    let mut r = request("missing", &c);
    r.sku = "MISSING";
    assert!(matches!(
        record_publication(&pool, &author(), &FixedClock(1100), r).await,
        Err(PublicationError::NotFound)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn retries_bind_full_intent_actor_locale_and_expected_history(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    let c = choice();
    record_publication(&pool, &author(), &FixedClock(1100), request("once", &c))
        .await
        .expect("publish");
    pool.execute("INSERT INTO staff_capabilities VALUES('other','catalogue.manage')")
        .await
        .expect("other publisher");
    for field in 0..7 {
        let mut r = request("once", &c);
        match field {
            0 => r.sku = "OTHER",
            1 => r.locale = Locale::Fr,
            2 => r.expected_revision = 1,
            3 => r.choice = &PublicationChoice::Withdraw {},
            4 => r.evidence_ref = "changed",
            5 => r.reason = "changed",
            _ => {}
        }
        let token = if field == 6 {
            SessionToken::from_bytes([3; 32])
        } else {
            author()
        };
        // A changed non-Dutch locale needs a structurally valid source selection
        // before its full-intent conflict can be tested.
        let mut translated = selected();
        translated.source_review_revision = Some(1);
        let translated = PublicationChoice::Activate {
            selection: translated,
        };
        if field == 1 {
            r.choice = &translated;
        }
        assert!(
            matches!(
                record_publication(&pool, &token, &FixedClock(1100), r).await,
                Err(PublicationError::Conflict)
            ),
            "field {field}"
        );
    }
    let mut changed = selected();
    changed.family_slug = "changed-room".into();
    assert!(matches!(
        record_publication(
            &pool,
            &author(),
            &FixedClock(1100),
            request("once", &PublicationChoice::Activate { selection: changed })
        )
        .await,
        Err(PublicationError::Conflict)
    ));
    let mut stale = request("stale", &c);
    stale.expected_revision = 0;
    assert!(matches!(
        record_publication(&pool, &author(), &FixedClock(1100), stale).await,
        Err(PublicationError::Conflict)
    ));
    assert!(
        record_publication(&pool, &author(), &FixedClock(1100), request("once", &c))
            .await
            .expect("receipt")
            .replayed
    );
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_publishers_and_exact_retries_apply_one_complete_effect(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    let c = choice();
    let token = author();
    let (a, b) = tokio::join!(
        record_publication(&pool, &token, &FixedClock(1100), request("same", &c)),
        record_publication(&pool, &token, &FixedClock(1100), request("same", &c))
    );
    assert_ne!(a.expect("first").replayed, b.expect("second").replayed);
    let mut a = request("a", &PublicationChoice::Withdraw {});
    a.expected_revision = 1;
    let mut b = request("b", &PublicationChoice::Withdraw {});
    b.expected_revision = 1;
    let (a, b) = tokio::join!(
        record_publication(&pool, &token, &FixedClock(1100), a),
        record_publication(&pool, &token, &FixedClock(1100), b)
    );
    assert_eq!([a.is_ok(), b.is_ok()].into_iter().filter(|v| *v).count(), 1);
    assert_eq!(counts(&pool).await, (2, 1));
    assert!(!status(&pool, Locale::Nl).await.eligible);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn withdrawn_or_reapproved_decisions_never_silently_reactivate_publications(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    let mut s = selected();
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("initial", &choice()),
    )
    .await
    .expect("activate");
    for category in 0..3 {
        match category {
            0 => facts_review(&pool, s.facts, 1, FactDecision::Rejected, "facts-rejected").await,
            1 => {
                image_review(
                    &pool,
                    &s.image,
                    1,
                    ImageDecision::Rejected,
                    "image-rejected",
                )
                .await
            }
            _ => {
                copy_review(
                    &pool,
                    Locale::Nl,
                    1,
                    1,
                    TranslationDecision::Rejected,
                    "copy-rejected",
                )
                .await
            }
        }
        assert!(!status(&pool, Locale::Nl).await.eligible);
        let c = PublicationChoice::Activate {
            selection: s.clone(),
        };
        let mut r = request("stale-decision", &c);
        r.expected_revision = category + 1;
        assert!(matches!(
            record_publication(&pool, &author(), &FixedClock(1100), r).await,
            Err(PublicationError::NotEligible)
        ));
        match category {
            0 => {
                facts_review(
                    &pool,
                    s.facts,
                    2,
                    FactDecision::Approved,
                    "facts-reapproved",
                )
                .await;
                s.factual_review_revision = 3;
            }
            1 => {
                image_review(
                    &pool,
                    &s.image,
                    2,
                    ImageDecision::Approved,
                    "image-reapproved",
                )
                .await;
                s.image_review_revision = 3;
            }
            _ => {
                copy_review(
                    &pool,
                    Locale::Nl,
                    1,
                    2,
                    TranslationDecision::Approved,
                    "copy-reapproved",
                )
                .await;
                s.translation_review_revision = 3;
            }
        }
        assert!(!status(&pool, Locale::Nl).await.eligible);
        let c = PublicationChoice::Activate {
            selection: s.clone(),
        };
        let key = format!("republish-{category}");
        let mut r = request(&key, &c);
        r.expected_revision = category + 1;
        record_publication(&pool, &author(), &FixedClock(1100), r)
            .await
            .expect("explicit republish");
        assert!(status(&pool, Locale::Nl).await.eligible);
    }
    rights(&pool, 1, &MediaRightsChange::Revoke).await;
    assert!(!status(&pool, Locale::Nl).await.eligible);
    rights(
        &pool,
        2,
        &MediaRightsChange::Grant {
            use_scope: MediaUse::ProductPhotography,
            valid_from: 1100,
            term: RightsTerm::Perpetual,
            permission_ref: "new isolated grant".into(),
        },
    )
    .await;
    assert!(!status(&pool, Locale::Nl).await.eligible);
    s.image.rights_revision = 3;
    s.image_review_revision = 1;
    image_review(
        &pool,
        &s.image,
        0,
        ImageDecision::Approved,
        "new-rights-image-review",
    )
    .await;
    let c = PublicationChoice::Activate { selection: s };
    let mut r = request("new-rights-publication", &c);
    r.expected_revision = 4;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("new rights consumed explicitly");
    assert!(status(&pool, Locale::Nl).await.eligible);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn new_price_copy_specification_and_bom_drafts_invalidate_without_fallback(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    let mut s = selected();
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("initial", &choice()),
    )
    .await
    .expect("activate");
    price(&pool, 1, "price-2").await;
    assert!(!status(&pool, Locale::Nl).await.eligible);
    s.facts.price_revision = 2;
    facts_review(&pool, s.facts, 0, FactDecision::Approved, "facts-price-2").await;
    let c = PublicationChoice::Activate {
        selection: s.clone(),
    };
    let mut r = request("price-2-publication", &c);
    r.expected_revision = 1;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("new price");
    copy_draft(&pool, Locale::Nl, 1, None, 1, "copy-2").await;
    assert!(!status(&pool, Locale::Nl).await.eligible);
    s.translation_revision = 2;
    let c = PublicationChoice::Activate {
        selection: s.clone(),
    };
    let mut r = request("unreviewed-copy", &c);
    r.expected_revision = 2;
    assert!(matches!(
        record_publication(&pool, &author(), &FixedClock(1100), r).await,
        Err(PublicationError::NotEligible)
    ));
    copy_review(
        &pool,
        Locale::Nl,
        2,
        0,
        TranslationDecision::Approved,
        "copy-2-review",
    )
    .await;
    let c = PublicationChoice::Activate {
        selection: s.clone(),
    };
    let mut r = request("copy-2-publication", &c);
    r.expected_revision = 2;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("new copy");
    specification(&pool, 1, 1, "spec-2").await;
    assert!(!status(&pool, Locale::Nl).await.eligible);
    s.facts.specification_revision = 2;
    s.image.specification_revision = 2;
    facts_review(&pool, s.facts, 0, FactDecision::Approved, "facts-spec-2").await;
    image_review(&pool, &s.image, 0, ImageDecision::Approved, "image-spec-2").await;
    let c = PublicationChoice::Activate {
        selection: s.clone(),
    };
    let mut r = request("mismatched-copy", &c);
    r.expected_revision = 3;
    assert!(matches!(
        record_publication(&pool, &author(), &FixedClock(1100), r).await,
        Err(PublicationError::NotEligible)
    ));
    copy_draft(&pool, Locale::Nl, 2, None, 2, "copy-3").await;
    copy_review(
        &pool,
        Locale::Nl,
        3,
        0,
        TranslationDecision::Approved,
        "copy-3-review",
    )
    .await;
    s.translation_revision = 3;
    let c = PublicationChoice::Activate {
        selection: s.clone(),
    };
    let mut r = request("spec-2-publication", &c);
    r.expected_revision = 3;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("new specification");
    definition(
        &pool,
        &CatalogueCommand::Bom {
            sku: "SET-FILLED".into(),
            expected_revision: 1,
            lines: lines(3),
        },
        "bom-2",
    )
    .await;
    assert!(!status(&pool, Locale::Nl).await.eligible);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn translated_publication_requires_the_exact_current_approved_dutch_source(pool: PgPool) {
    let _case = media_seed(&pool).await;
    facts_review(&pool, selected().facts, 0, FactDecision::Approved, "facts").await;
    image_review(
        &pool,
        &selected().image,
        0,
        ImageDecision::Approved,
        "image",
    )
    .await;
    copy_draft(&pool, Locale::Nl, 1, None, 0, "nl").await;
    copy_draft(&pool, Locale::De, 1, Some(1), 0, "de").await;
    copy_review(
        &pool,
        Locale::De,
        1,
        0,
        TranslationDecision::Approved,
        "de-review",
    )
    .await;
    let mut s = selected();
    s.source_review_revision = Some(1);
    let c = PublicationChoice::Activate {
        selection: s.clone(),
    };
    let mut r = request("missing-source-review", &c);
    r.locale = Locale::De;
    assert!(matches!(
        record_publication(&pool, &author(), &FixedClock(1100), r).await,
        Err(PublicationError::NotEligible)
    ));
    assert!(sqlx::query("INSERT INTO catalogue_publication_revisions(sku,locale,revision,effect_key,choice,evidence_ref,actor,reason,evaluated_at) VALUES('SET-FILLED','de',1,'raw-missing-dutch',$1::jsonb,'isolated','author','isolated',1100)").bind(serde_json::to_string(&c).expect("choice")).execute(&pool).await.is_err());
    copy_review(
        &pool,
        Locale::Nl,
        1,
        0,
        TranslationDecision::Approved,
        "nl-review",
    )
    .await;
    let mut r = request("de-publication", &c);
    r.locale = Locale::De;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("translated publication");
    assert!(status(&pool, Locale::De).await.eligible);
    copy_review(
        &pool,
        Locale::Nl,
        1,
        1,
        TranslationDecision::Rejected,
        "source-withdrawal",
    )
    .await;
    assert!(!status(&pool, Locale::De).await.eligible);
    copy_review(
        &pool,
        Locale::Nl,
        1,
        2,
        TranslationDecision::Approved,
        "source-reapproval",
    )
    .await;
    assert!(!status(&pool, Locale::De).await.eligible);
    s.source_review_revision = Some(3);
    let c = PublicationChoice::Activate { selection: s };
    let mut r = request("source-3-publication", &c);
    r.locale = Locale::De;
    r.expected_revision = 1;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("explicit new source approval");
    assert!(status(&pool, Locale::De).await.eligible);
    let mut r = request("missing-fr", &c);
    r.locale = Locale::Fr;
    assert!(matches!(
        record_publication(&pool, &author(), &FixedClock(1100), r).await,
        Err(PublicationError::NotEligible)
    ));
    assert!(
        publication_status(
            &pool,
            &author(),
            &FixedClock(1100),
            "SET-FILLED",
            Locale::Fr
        )
        .await
        .expect("no fallback")
        .is_none()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unsafe_public_money_and_wrong_review_references_cannot_activate(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    for field in 0..3 {
        let mut s = selected();
        match field {
            0 => s.factual_review_revision = 2,
            1 => s.image_review_revision = 2,
            _ => s.translation_review_revision = 2,
        };
        let c = PublicationChoice::Activate { selection: s };
        assert!(matches!(
            record_publication(
                &pool,
                &author(),
                &FixedClock(1100),
                request("wrong-review", &c)
            )
            .await,
            Err(PublicationError::NotEligible)
        ));
    }
    record_price_revision(
        &pool,
        &author(),
        &FixedClock(1100),
        PriceRevisionRequest {
            sku: "SET-FILLED",
            expected_revision: 1,
            effect_key: "unsafe-price",
            price: commerce_domain::Money::eur(9_007_199_254_740_992).expect("integer"),
            evidence_ref: "isolated bound",
            reason: "isolated",
            confirmed: true,
        },
    )
    .await
    .expect("private draft");
    let mut s = selected();
    s.facts.price_revision = 2;
    facts_review(
        &pool,
        s.facts,
        0,
        FactDecision::Approved,
        "huge-facts-review",
    )
    .await;
    let c = PublicationChoice::Activate { selection: s };
    assert!(matches!(
        record_publication(
            &pool,
            &author(),
            &FixedClock(1100),
            request("unsafe-public-money", &c)
        )
        .await,
        Err(PublicationError::NotEligible)
    ));
    assert!(
        raw(
            &pool,
            &serde_json::to_value(&c).expect("choice"),
            1,
            "unsafe-import"
        )
        .await
        .is_err()
    );
    assert_eq!(counts(&pool).await, (0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn raw_schema_rejects_unapproved_open_shapes_gaps_and_route_rebinding(pool: PgPool) {
    let _case = media_seed(&pool).await;
    let good = serde_json::to_value(choice()).expect("choice");
    assert!(raw(&pool, &good, 1, "unapproved").await.is_err());
    approve(&pool).await;
    for field in 0..10 {
        let mut bad = good.clone();
        match field {
            0 => bad["actor"] = serde_json::json!("owner"),
            1 => bad["selection"]["unknown"] = serde_json::json!(true),
            2 => bad["selection"]["facts"]["unknown"] = serde_json::json!(true),
            3 => bad["selection"]["image"]["unknown"] = serde_json::json!(true),
            4 => bad["selection"]["image"]["asset_id"] = serde_json::json!(1),
            5 => bad["selection"]["family_slug"] = serde_json::json!(12),
            6 => bad["selection"]["family_slug"] = serde_json::json!("../bad"),
            7 => bad["selection"]["factual_review_revision"] = serde_json::json!("1"),
            8 => bad["selection"]["facts"]["price_revision"] = serde_json::json!(1.0),
            _ => {
                bad["selection"]
                    .as_object_mut()
                    .expect("object")
                    .remove("source_review_revision");
            }
        }
        assert!(
            raw(&pool, &bad, 1, &format!("bad-{field}")).await.is_err(),
            "field {field}"
        );
    }
    assert!(raw(&pool, &good, 2, "gap").await.is_err());
    assert_eq!(counts(&pool).await, (0, 0));
    raw(&pool, &good, 1, "imported")
        .await
        .expect("exact valid import");
    for sql in [
        "UPDATE catalogue_publication_revisions SET reason='changed'",
        "DELETE FROM catalogue_publication_revisions",
        "UPDATE catalogue_family_routes SET slug='changed'",
        "DELETE FROM catalogue_family_routes",
    ] {
        assert!(pool.execute(sql).await.is_err());
    }
    let mut s = selected();
    s.family_slug = "changed-room".into();
    let c = PublicationChoice::Activate { selection: s };
    let mut r = request("rebind", &c);
    r.expected_revision = 1;
    assert!(matches!(
        record_publication(&pool, &author(), &FixedClock(1100), r).await,
        Err(PublicationError::Conflict)
    ));
    assert!(
        raw(
            &pool,
            &serde_json::to_value(c).expect("changed"),
            2,
            "raw-rebind"
        )
        .await
        .is_err()
    );
    let mut withdrawal = serde_json::json!({"action":"withdraw"});
    withdrawal["selection"] = good["selection"].clone();
    assert!(raw(&pool, &withdrawal, 2, "open-withdraw").await.is_err());
    assert!(pool.execute("INSERT INTO catalogue_family_routes(family_code,locale,slug,creation_effect_key) VALUES('DEMO-FAMILY','de','forged','imported')").await.is_err());
    assert_eq!(counts(&pool).await, (1, 1));
    assert!(status(&pool, Locale::Nl).await.eligible);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn final_session_and_licence_clocks_roll_back_records_and_route_assignment(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    let c = choice();
    for (after, value) in [
        (1, Ok(4600)),
        (2, Ok(2000)),
        (2, Err(ContextError::ClockUnavailable)),
        (2, Ok(u64::MAX)),
    ] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            after,
            value,
        };
        let error = record_publication(&pool, &author(), &clock, request("clock", &c))
            .await
            .expect_err("no late activation");
        if value == Ok(2000) {
            assert!(matches!(error, PublicationError::NotEligible));
        } else {
            assert!(matches!(error, PublicationError::Staff(_)));
        }
        assert_eq!(counts(&pool).await, (0, 0));
    }
    record_publication(&pool, &author(), &FixedClock(1100), request("clock", &c))
        .await
        .expect("valid clock");
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        after: 1,
        value: Ok(4600),
    };
    assert!(matches!(
        record_publication(&pool, &author(), &clock, request("clock", &c)).await,
        Err(PublicationError::Staff(StaffError::Denied))
    ));
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        after: 1,
        value: Ok(2000),
    };
    let expired = publication_status(&pool, &author(), &clock, "SET-FILLED", Locale::Nl)
        .await
        .expect("fresh licence evaluation")
        .expect("history");
    assert!(!expired.eligible);
    assert_eq!(expired.evaluated_at, 2000);
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        after: 1,
        value: Err(ContextError::ClockUnavailable),
    };
    assert!(matches!(
        publication_status(&pool, &author(), &clock, "SET-FILLED", Locale::Nl).await,
        Err(PublicationError::Staff(StaffError::ClockUnavailable))
    ));
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        after: 1,
        value: Err(ContextError::ClockUnavailable),
    };
    assert!(
        publication_status(&pool, &author(), &clock, "MISSING", Locale::Nl)
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = false)]
async fn upgrade_preserves_all_sources_and_starts_without_publications_or_paths(pool: PgPool) {
    MIGRATOR.run_to(15, &pool).await.expect("prior schema");
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    let old = commerce_infrastructure::catalogue(&pool, Locale::Nl)
        .await
        .expect("old reader");
    MIGRATOR.run(&pool).await.expect("upgrade");
    assert_eq!(counts(&pool).await, (0, 0));
    assert_eq!(
        old,
        commerce_infrastructure::catalogue(&pool, Locale::Nl)
            .await
            .expect("retained reader")
    );
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("explicit-after-upgrade", &choice()),
    )
    .await
    .expect("explicit activation");
    assert!(status(&pool, Locale::Nl).await.eligible);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_waiting_status_reader_observes_committed_withdrawal_before_returning(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("active", &choice()),
    )
    .await
    .expect("active");
    let mut withdrawal = pool.begin().await.expect("withdrawal");
    withdrawal
        .execute("UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku='SET-FILLED'")
        .await
        .expect("serialize");
    withdrawal.execute("INSERT INTO catalogue_publication_revisions(sku,locale,revision,effect_key,choice,evidence_ref,actor,reason,evaluated_at) VALUES('SET-FILLED','nl',2,'withdraw-wait','{\"action\":\"withdraw\"}','isolated','author','isolated',1100)").await.expect("pending withdrawal");
    let owned = pool.clone();
    let reader = tokio::spawn(async move {
        publication_status(
            &owned,
            &author(),
            &FixedClock(1100),
            "SET-FILLED",
            Locale::Nl,
        )
        .await
    });
    waiting_on(&pool, "SELECT sku FROM catalogue_variants%").await;
    withdrawal.commit().await.expect("withdraw");
    let observed = reader
        .await
        .expect("task")
        .expect("status")
        .expect("history");
    assert_eq!(observed.record.revision, 2);
    assert!(!observed.eligible);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_and_cancellation_through_real_insert_waits_leave_no_partial_paths(pool: PgPool) {
    struct MovingClock(std::sync::atomic::AtomicU64);
    impl Clock for MovingClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    for cancel in [false, true] {
        let mut blocker = pool.begin().await.expect("blocker");
        blocker
            .execute("LOCK TABLE catalogue_publication_revisions IN SHARE MODE")
            .await
            .expect("block insert");
        let clock = std::sync::Arc::new(MovingClock(std::sync::atomic::AtomicU64::new(1100)));
        let owned_clock = clock.clone();
        let owned = pool.clone();
        let task = tokio::spawn(async move {
            record_publication(
                &owned,
                &author(),
                owned_clock.as_ref(),
                request("waiting", &choice()),
            )
            .await
        });
        waiting_on(&pool, "INSERT INTO catalogue_publication_revisions%").await;
        if cancel {
            task.abort();
            assert!(task.await.expect_err("cancelled").is_cancelled());
            blocker.rollback().await.expect("release");
        } else {
            clock.0.store(2000, Ordering::SeqCst);
            blocker.rollback().await.expect("release");
            assert!(matches!(
                task.await.expect("task"),
                Err(PublicationError::NotEligible)
            ));
        }
        assert_eq!(counts(&pool).await, (0, 0));
    }
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("waiting", &choice()),
    )
    .await
    .expect("retry after rollback");
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn grant_revocation_waits_for_atomic_activation_then_denies_reads_and_replays(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute("LOCK TABLE catalogue_publication_revisions IN SHARE MODE")
        .await
        .expect("block insert");
    let owned = pool.clone();
    let writer = tokio::spawn(async move {
        record_publication(
            &owned,
            &author(),
            &FixedClock(1100),
            request("authorized", &choice()),
        )
        .await
    });
    waiting_on(&pool, "INSERT INTO catalogue_publication_revisions%").await;
    let owned = pool.clone();
    let revoke = tokio::spawn(async move {
        owned.execute("DELETE FROM staff_capabilities WHERE staff_id='author' AND capability='catalogue.manage'").await
    });
    waiting_on(&pool, "DELETE FROM staff_capabilities%").await;
    assert!(!revoke.is_finished());
    blocker.rollback().await.expect("resume");
    writer.await.expect("writer task").expect("commit");
    revoke.await.expect("revoke task").expect("revoked");
    assert!(matches!(
        publication_status(
            &pool,
            &author(),
            &FixedClock(1100),
            "SET-FILLED",
            Locale::Nl
        )
        .await,
        Err(PublicationError::Staff(StaffError::Denied))
    ));
    assert!(matches!(
        record_publication(
            &pool,
            &author(),
            &FixedClock(1100),
            request("authorized", &choice())
        )
        .await,
        Err(PublicationError::Staff(StaffError::Denied))
    ));
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_repeatable_read_imports_cannot_ignore_price_or_permission_changes(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    for rights_change in [false, true] {
        let mut stale = pool.begin().await.expect("snapshot");
        stale
            .execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .await
            .expect("isolation");
        stale
            .execute("SELECT * FROM catalogue_media_rights")
            .await
            .expect("capture");
        if rights_change {
            rights(&pool, 1, &MediaRightsChange::Revoke).await;
        } else {
            price(&pool, 1, "new-price").await;
        }
        let error=sqlx::query("INSERT INTO catalogue_publication_revisions(sku,locale,revision,effect_key,choice,evidence_ref,actor,reason,evaluated_at) VALUES('SET-FILLED','nl',1,'stale',$1::jsonb,'isolated','author','isolated',1100)").bind(serde_json::to_string(&choice()).expect("choice")).execute(&mut *stale).await.expect_err("stale import must retry");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("40001")
        );
        stale.rollback().await.expect("rollback");
    }
    assert_eq!(counts(&pool).await, (0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bounded_effect_waits_and_missing_targets_never_leave_activation_fragments(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute(
            "SELECT pg_advisory_xact_lock(hashtextextended('catalogue-publication:bounded',0))",
        )
        .await
        .expect("lock");
    let error = record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("bounded", &choice()),
    )
    .await
    .expect_err("bounded wait");
    assert!(
        matches!(error,PublicationError::Database(ref e) if e.as_database_error().and_then(|e|e.code()).as_deref()==Some("55P03"))
    );
    blocker.rollback().await.expect("release");
    assert_eq!(counts(&pool).await, (0, 0));
    for field in 0..2 {
        let mut s = selected();
        if field == 0 {
            s.facts.price_revision = 99;
        } else {
            s.image.asset_id = "missing".into();
        }
        let c = PublicationChoice::Activate { selection: s };
        assert!(matches!(
            record_publication(&pool, &author(), &FixedClock(1100), request("missing", &c)).await,
            Err(PublicationError::NotEligible)
        ));
    }
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("bounded", &choice()),
    )
    .await
    .expect("retry");
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn corrupted_source_payloads_never_form_eligible_status_or_new_activation(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("active", &choice()),
    )
    .await
    .expect("active");
    let spec:String=sqlx::query_scalar("SELECT payload::text FROM catalogue_specification_revisions WHERE sku='SET-FILLED' AND revision=1").fetch_one(&pool).await.expect("original");
    pool.execute("ALTER TABLE catalogue_specification_revisions DISABLE TRIGGER USER; UPDATE catalogue_specification_revisions SET payload='{}' WHERE sku='SET-FILLED'").await.expect("corrupt isolated source");
    assert!(matches!(
        publication_status(
            &pool,
            &author(),
            &FixedClock(1100),
            "SET-FILLED",
            Locale::Nl
        )
        .await,
        Err(PublicationError::Facts(_))
    ));
    let c = choice();
    let mut r = request("bad-spec", &c);
    r.expected_revision = 1;
    assert!(matches!(
        record_publication(&pool, &author(), &FixedClock(1100), r).await,
        Err(PublicationError::Facts(_))
    ));
    sqlx::query("UPDATE catalogue_specification_revisions SET payload=$1::jsonb WHERE sku='SET-FILLED' AND revision=1").bind(spec).execute(&pool).await.expect("restore spec");
    pool.execute("ALTER TABLE catalogue_specification_revisions ENABLE TRIGGER USER")
        .await
        .expect("restore guards");
    let rights: String = sqlx::query_scalar(
        "SELECT change::text FROM catalogue_media_rights WHERE asset_id='photo' AND revision=1",
    )
    .fetch_one(&pool)
    .await
    .expect("rights");
    pool.execute("ALTER TABLE catalogue_media_rights DISABLE TRIGGER USER; UPDATE catalogue_media_rights SET change='{}'").await.expect("corrupt rights");
    assert!(matches!(
        publication_status(
            &pool,
            &author(),
            &FixedClock(1100),
            "SET-FILLED",
            Locale::Nl
        )
        .await,
        Err(PublicationError::Image(_))
    ));
    sqlx::query(
        "UPDATE catalogue_media_rights SET change=$1::jsonb WHERE asset_id='photo' AND revision=1",
    )
    .bind(rights)
    .execute(&pool)
    .await
    .expect("restore rights");
    pool.execute("ALTER TABLE catalogue_media_rights ENABLE TRIGGER USER")
        .await
        .expect("restore guards");
    let copy:String=sqlx::query_scalar("SELECT payload::text FROM catalogue_translation_revisions WHERE sku='SET-FILLED' AND locale='nl' AND revision=1").fetch_one(&pool).await.expect("copy");
    pool.execute("ALTER TABLE catalogue_translation_revisions DISABLE TRIGGER USER; UPDATE catalogue_translation_revisions SET payload='{}'").await.expect("corrupt copy");
    assert!(matches!(
        publication_status(
            &pool,
            &author(),
            &FixedClock(1100),
            "SET-FILLED",
            Locale::Nl
        )
        .await,
        Err(PublicationError::Copy(_))
    ));
    sqlx::query("UPDATE catalogue_translation_revisions SET payload=$1::jsonb WHERE sku='SET-FILLED' AND locale='nl' AND revision=1").bind(copy).execute(&pool).await.expect("restore copy");
    pool.execute("ALTER TABLE catalogue_translation_revisions ENABLE TRIGGER USER")
        .await
        .expect("restore guards");
    assert!(status(&pool, Locale::Nl).await.eligible);
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn corrupt_publication_records_missing_sources_and_missing_asset_claims_fail_closed(
    pool: PgPool,
) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("active", &choice()),
    )
    .await
    .expect("active");
    pool.execute("ALTER TABLE catalogue_publication_revisions DISABLE TRIGGER USER; UPDATE catalogue_publication_revisions SET choice=choice || '{\"unknown\":true}'").await.expect("corrupt history");
    assert!(matches!(
        publication_status(
            &pool,
            &author(),
            &FixedClock(1100),
            "SET-FILLED",
            Locale::Nl
        )
        .await,
        Err(PublicationError::InvalidStoredData)
    ));
    assert!(matches!(
        record_publication(
            &pool,
            &author(),
            &FixedClock(1100),
            request("active", &choice())
        )
        .await,
        Err(PublicationError::InvalidStoredData)
    ));
    pool.execute("UPDATE catalogue_publication_revisions SET choice=jsonb_set(choice-'unknown','{selection,facts,price_revision}','99')").await.expect("missing source binding");
    assert!(!status(&pool, Locale::Nl).await.eligible);
    sqlx::query("UPDATE catalogue_publication_revisions SET choice=$1::jsonb")
        .bind(serde_json::to_string(&choice()).expect("choice"))
        .execute(&pool)
        .await
        .expect("restore exact record");
    pool.execute("UPDATE catalogue_publication_revisions SET choice=jsonb_set(choice,'{selection,family_slug}','\"wrong-room\"')").await.expect("corrupt route reference");
    assert!(matches!(
        publication_status(
            &pool,
            &author(),
            &FixedClock(1100),
            "SET-FILLED",
            Locale::Nl
        )
        .await,
        Err(PublicationError::InvalidStoredData)
    ));
    sqlx::query("UPDATE catalogue_publication_revisions SET choice=$1::jsonb")
        .bind(serde_json::to_string(&choice()).expect("choice"))
        .execute(&pool)
        .await
        .expect("restore route reference");
    pool.execute("ALTER TABLE catalogue_publication_revisions ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    pool.execute("ALTER TABLE catalogue_media_assets DROP CONSTRAINT media_serialization_required; DELETE FROM catalogue_media_serialization WHERE asset_id='photo'").await.expect("missing claim diagnostic");
    assert!(matches!(
        publication_status(
            &pool,
            &author(),
            &FixedClock(1100),
            "SET-FILLED",
            Locale::Nl
        )
        .await,
        Err(PublicationError::InvalidStoredData)
    ));
    pool.execute("INSERT INTO catalogue_media_serialization VALUES('photo',true); ALTER TABLE catalogue_media_assets ADD CONSTRAINT media_serialization_required FOREIGN KEY(asset_id) REFERENCES catalogue_media_serialization(asset_id) DEFERRABLE INITIALLY DEFERRED").await.expect("restore relationship");
    assert!(status(&pool, Locale::Nl).await.eligible);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn eligibility_query_waits_cannot_return_expired_authority_or_photo_rights(pool: PgPool) {
    struct MovingClock(std::sync::atomic::AtomicU64);
    impl Clock for MovingClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("active", &choice()),
    )
    .await
    .expect("active");
    // Instrument a real PostgreSQL dependency wait, preserving the original predicate.
    // A fresh pool ensures statement caches resolve the wrapper's new function OID.
    pool.execute("ALTER FUNCTION publication_sources_current(text,text,jsonb,bigint) RENAME TO original_publication_sources_current; CREATE FUNCTION publication_sources_current(item text,language text,choice jsonb,now_seconds bigint) RETURNS boolean LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(hashtextextended('publication-late-read',0)); RETURN original_publication_sources_current(item,language,choice,now_seconds); END; $$").await.expect("isolated wait instrumentation");
    let fresh = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .expect("fresh statement cache");
    for final_time in [2000, 2001] {
        let mut blocker = pool.begin().await.expect("blocker");
        blocker
            .execute("SELECT pg_advisory_xact_lock(hashtextextended('publication-late-read',0))")
            .await
            .expect("block predicate");
        let clock = std::sync::Arc::new(MovingClock(std::sync::atomic::AtomicU64::new(1100)));
        let owned_clock = clock.clone();
        let owned = fresh.clone();
        let reader = tokio::spawn(async move {
            publication_status(
                &owned,
                &author(),
                owned_clock.as_ref(),
                "SET-FILLED",
                Locale::Nl,
            )
            .await
        });
        waiting_on(&pool, "SELECT publication_sources_current%").await;
        clock.0.store(final_time, Ordering::SeqCst);
        blocker.rollback().await.expect("resume predicate");
        let result = reader.await.expect("reader task");
        if final_time == 2000 {
            let status = result.expect("authority still current").expect("history");
            assert!(!status.eligible);
            assert_eq!(status.evaluated_at, 2000);
        } else {
            assert!(matches!(
                result,
                Err(PublicationError::Staff(StaffError::Denied))
            ));
        }
    }
    fresh.close().await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn assigned_paths_require_their_original_activation_binding_on_read_and_reuse(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("active", &choice()),
    )
    .await
    .expect("active");
    assert!(pool.execute("INSERT INTO catalogue_family_routes(family_code,locale,slug,creation_effect_key) VALUES('ROOM','de','wrong-locale','active')").await.is_err());
    let mut withdrawal = request("de-withdraw", &PublicationChoice::Withdraw {});
    withdrawal.locale = Locale::De;
    record_publication(&pool, &author(), &FixedClock(1100), withdrawal)
        .await
        .expect("separate withdrawal");
    pool.execute("ALTER TABLE catalogue_family_routes DISABLE TRIGGER USER; UPDATE catalogue_family_routes SET creation_effect_key='de-withdraw'").await.expect("isolated binding corruption");
    assert!(matches!(
        publication_status(
            &pool,
            &author(),
            &FixedClock(1100),
            "SET-FILLED",
            Locale::Nl
        )
        .await,
        Err(PublicationError::InvalidStoredData)
    ));
    let c = choice();
    let mut r = request("invalid-route-reuse", &c);
    r.expected_revision = 1;
    assert!(matches!(
        record_publication(&pool, &author(), &FixedClock(1100), r).await,
        Err(PublicationError::Database(_))
    ));
    assert_eq!(counts(&pool).await, (2, 1));
    pool.execute("UPDATE catalogue_family_routes SET creation_effect_key='active'")
        .await
        .expect("restore binding");
    pool.execute("ALTER TABLE catalogue_family_routes ENABLE TRIGGER USER")
        .await
        .expect("restore guards after deferred FK events complete");
    assert!(status(&pool, Locale::Nl).await.eligible);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn deferred_constraint_waits_finish_before_final_authority_and_licence_checks(pool: PgPool) {
    struct MovingClock(std::sync::atomic::AtomicU64);
    impl Clock for MovingClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    pool.execute("CREATE FUNCTION wait_publication_constraint() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(hashtextextended('publication-deferred-clock',0)); RETURN NULL; END; $$; CREATE CONSTRAINT TRIGGER wait_publication_constraint AFTER INSERT ON catalogue_publication_revisions DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION wait_publication_constraint()").await.expect("isolated deferred wait instrument");
    for final_time in [2000, 2001] {
        let mut blocker = pool.begin().await.expect("blocker");
        blocker
            .execute(
                "SELECT pg_advisory_xact_lock(hashtextextended('publication-deferred-clock',0))",
            )
            .await
            .expect("block deferred work");
        let clock = std::sync::Arc::new(MovingClock(std::sync::atomic::AtomicU64::new(1100)));
        let owned_clock = clock.clone();
        let owned = pool.clone();
        let writer = tokio::spawn(async move {
            record_publication(
                &owned,
                &author(),
                owned_clock.as_ref(),
                request("deferred", &choice()),
            )
            .await
        });
        waiting_on(&pool, "%").await;
        clock.0.store(final_time, Ordering::SeqCst);
        blocker.rollback().await.expect("resume constraint");
        let result = writer.await.expect("writer task");
        if final_time == 2000 {
            assert!(matches!(result, Err(PublicationError::NotEligible)));
        } else {
            assert!(matches!(
                result,
                Err(PublicationError::Staff(StaffError::Denied))
            ));
        }
        assert_eq!(counts(&pool).await, (0, 0));
    }
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("deferred", &choice()),
    )
    .await
    .expect("retry after exact rollback");
    assert_eq!(counts(&pool).await, (1, 1));
}

async fn waiting_on(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(800),async {loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(pattern).fetch_one(pool).await.expect("wait oracle");
        if waiting {break;}tokio::task::yield_now().await;
    }}).await.expect("actual database lock wait");
}
