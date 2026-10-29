// Synthetic merchant candidates/bytes in disposable PG only, never actual supplier facts.
use commerce_application::request_context::{Clock, ContextError};
use commerce_domain::{
    ComponentKind,
    catalogue_authoring::{BomLineDraft, CatalogueCommand, Fill},
    image_review::{ImageDecision, ImageTarget},
    media::{ImageType, MediaAssetDraft, MediaKind, MediaRightsChange, MediaUse, RightsTerm},
};
use commerce_infrastructure::{
    MIGRATOR,
    catalogue_authoring::{DefinitionRequest, record_definition},
    catalogue_prices::{PriceRevisionRequest, record_price_revision},
    image_ingestion::{ImageError, IngestImageRequest, ingest_image, private_rendition},
    image_review::*,
    media::{MediaRightsRequest, RegisterMediaRequest, record_media_rights, register_media},
    specifications::{SpecificationRequest, record_specification},
    staff_access::{SessionToken, StaffError},
};
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgPool};
use std::{
    io::Cursor,
    sync::atomic::{AtomicUsize, Ordering},
};
include!("../../../tests/fixtures/specification.rs");
include!("support/image_review_seed.rs");
struct FixedClock(u64);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
struct StepClock {
    calls: AtomicUsize,
    after: usize,
    value: Result<u64, ContextError>,
}
impl Clock for StepClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) < self.after {
            Ok(1100)
        } else {
            self.value
        }
    }
}
fn author() -> SessionToken {
    SessionToken::from_bytes([1; 32])
}
fn reviewer() -> SessionToken {
    SessionToken::from_bytes([2; 32])
}
fn target() -> ImageTarget {
    ImageTarget {
        asset_id: "photo".into(),
        bom_revision: 1,
        specification_revision: 1,
        rights_revision: 1,
    }
}
fn request<'a>(key: &'a str, target: &'a ImageTarget) -> ImageReviewRequest<'a> {
    ImageReviewRequest {
        sku: "SET-FILLED",
        target,
        expected_revision: 0,
        effect_key: key,
        decision: ImageDecision::Approved,
        evidence_ref: "isolated visual/material/colour comparison",
        qualification_ref: "isolated qualification",
        reason: "isolated review",
        confirmed: true,
    }
}
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
async fn count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM catalogue_image_reviews")
        .fetch_one(pool)
        .await
        .expect("count")
}
async fn raw(pool: &PgPool, decision: &str, revision: i32, actor: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO catalogue_image_reviews(sku,asset_id,bom_revision,specification_revision,rights_revision,revision,effect_key,decision,evidence_ref,qualification_ref,actor,reason,evaluated_at) VALUES('SET-FILLED','photo',1,1,1,$1,$2,$3,'raw evidence','raw qualification',$4,'raw reason',1100)")
        .bind(revision).bind(format!("raw-{revision}-{actor}" )).bind(decision).bind(actor).execute(pool).await.map(|_|())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn reviewers_inspect_exact_sources_and_safe_bytes_without_manage_or_publication(
    pool: PgPool,
) {
    let _case = media_seed(&pool).await;
    let t = target();
    let bundle = image_review_bundle(&pool, &reviewer(), &FixedClock(1100), "SET-FILLED", &t)
        .await
        .expect("inspect")
        .expect("bundle");
    assert_eq!(bundle.identity.actor, "author");
    assert_eq!(bundle.specification.draft.footprint.width_mm, 1000);
    assert_eq!(bundle.image.recipe, "image-v1");
    assert_eq!(bundle.asset.sha256.len(), 32);
    assert!(bundle.review.is_none());
    let bytes = image_for_review(&pool, &reviewer(), &FixedClock(1100), "SET-FILLED", &t, 320)
        .await
        .expect("private")
        .expect("image");
    assert_eq!((bytes.width, bytes.height), (32, 24));
    assert_eq!(
        image::load_from_memory(&bytes.bytes)
            .expect("decode")
            .width(),
        32
    );
    assert!(matches!(
        private_rendition(&pool, &reviewer(), &FixedClock(1100), "photo", 320).await,
        Err(ImageError::Staff(StaffError::Denied))
    ));
    let receipt = record_image_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        request("approve", &t),
    )
    .await
    .expect("review");
    assert_eq!((receipt.revision, receipt.replayed), (1, false));
    let bundle = image_review_bundle(&pool, &reviewer(), &FixedClock(1100), "SET-FILLED", &t)
        .await
        .expect("inspect")
        .expect("bundle");
    assert_eq!(
        bundle.review.expect("decision").decision,
        ImageDecision::Approved
    );
    let unactivated: (Option<i64>, Option<String>) =
        sqlx::query_as("SELECT minor_units,slug FROM catalogue_items WHERE sku='SET-FILLED'")
            .fetch_one(&pool)
            .await
            .expect("candidate");
    assert_eq!(unactivated, (None, None));
    assert!(
        image_for_review(&pool, &reviewer(), &FixedClock(1100), "MISSING", &t, 320)
            .await
            .expect("absent")
            .is_none()
    );
    assert!(matches!(
        image_for_review(&pool, &reviewer(), &FixedClock(1100), "SET-FILLED", &t, 500).await,
        Err(ImageReviewError::Image(ImageError::InvalidRequest))
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn independent_author_confirmation_role_and_bounded_intent_are_mandatory(pool: PgPool) {
    let _case = media_seed(&pool).await;
    let t = target();
    for token in [
        author(),
        SessionToken::from_bytes([4; 32]),
        SessionToken::from_bytes([99; 32]),
    ] {
        assert!(matches!(
            record_image_review(&pool, &token, &FixedClock(1100), request("denied", &t)).await,
            Err(ImageReviewError::Staff(StaffError::Denied))
        ));
    }
    pool.execute("INSERT INTO staff_capabilities VALUES('author','catalogue.review')")
        .await
        .expect("dual grant");
    assert!(matches!(
        record_image_review(&pool, &author(), &FixedClock(1100), request("self", &t)).await,
        Err(ImageReviewError::SelfReview)
    ));
    for field in 0..8 {
        let mut r = request("invalid", &t);
        match field {
            0 => r.confirmed = false,
            1 => r.sku = "DEMO-FILLED",
            2 => r.expected_revision = -1,
            3 => r.effect_key = "../key",
            4 => r.evidence_ref = " ",
            5 => r.qualification_ref = " ",
            6 => r.reason = " ",
            _ => r.expected_revision = i32::MAX,
        }
        assert!(
            matches!(
                record_image_review(&pool, &reviewer(), &FixedClock(1100), r).await,
                Err(ImageReviewError::InvalidRequest)
            ),
            "field {field}"
        );
    }
    let mut invalid = t.clone();
    invalid.bom_revision = 0;
    assert!(matches!(
        image_review_bundle(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            "SET-FILLED",
            &invalid
        )
        .await,
        Err(ImageReviewError::InvalidRequest)
    ));

    assert!(matches!(
        record_image_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            request("invalid", &invalid)
        )
        .await,
        Err(ImageReviewError::InvalidRequest)
    ));
    assert!(matches!(
        image_review_bundle(&pool, &reviewer(), &FixedClock(1100), "bad/sku", &t).await,
        Err(ImageReviewError::InvalidRequest)
    ));
    assert_eq!(count(&pool).await, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_sources_rights_expiry_and_withdrawal_never_reactivate_old_approvals(pool: PgPool) {
    let _case = media_seed(&pool).await;
    let t = target();
    record_image_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        request("approve", &t),
    )
    .await
    .expect("approved");
    specification(&pool, 1, 1, "spec-2").await;
    let mut r = request("stale-spec", &t);
    r.expected_revision = 1;
    assert!(matches!(
        record_image_review(&pool, &reviewer(), &FixedClock(1100), r).await,
        Err(ImageReviewError::Conflict)
    ));
    rights(&pool, 1, &MediaRightsChange::Revoke).await;
    let mut r = request("withdraw", &t);
    r.expected_revision = 1;
    r.decision = ImageDecision::Rejected;
    assert_eq!(
        record_image_review(&pool, &reviewer(), &FixedClock(1100), r)
            .await
            .expect("withdraw")
            .revision,
        2
    );
    assert!(
        record_image_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            request("approve", &t)
        )
        .await
        .expect("historical receipt")
        .replayed
    );
    assert_eq!(
        image_review_bundle(&pool, &reviewer(), &FixedClock(1100), "SET-FILLED", &t)
            .await
            .expect("history")
            .expect("bundle")
            .review
            .expect("review")
            .decision,
        ImageDecision::Rejected
    );
    let mut current = t.clone();
    current.specification_revision = 2;
    current.rights_revision = 2;
    assert!(matches!(
        record_image_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            request("revoked", &current)
        )
        .await,
        Err(ImageReviewError::Conflict)
    ));
    rights(
        &pool,
        2,
        &MediaRightsChange::Grant {
            use_scope: MediaUse::ProductPhotography,
            valid_from: 1100,
            term: RightsTerm::Until { expires_at: 1200 },
            permission_ref: "synthetic permission".into(),
        },
    )
    .await;
    current.rights_revision = 3;
    assert!(matches!(
        record_image_review(
            &pool,
            &reviewer(),
            &FixedClock(1200),
            request("expired", &current)
        )
        .await,
        Err(ImageReviewError::Conflict)
    ));
    assert!(
        record_image_review(
            &pool,
            &reviewer(),
            &FixedClock(1199),
            request("fresh", &current)
        )
        .await
        .is_ok()
    );
}

#[test]
fn closed_targets_and_decisions_cannot_claim_publication_or_server_authority() {
    assert!(serde_json::from_str::<ImageTarget>(r#"{"asset_id":"photo","bom_revision":1,"specification_revision":1,"rights_revision":1,"actor":"owner"}"#).is_err());
    assert!(serde_json::from_str::<ImageDecision>(r#""published""#).is_err());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn each_source_change_requires_a_new_exact_review_but_price_does_not(pool: PgPool) {
    let _case = media_seed(&pool).await;
    let mut t = target();
    // Price is outside an image's visual/material suitability target.
    price(&pool, 1, "price-update").await;
    record_image_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        request("old-price", &t),
    )
    .await
    .expect("image not price");
    rights(
        &pool,
        1,
        &MediaRightsChange::Grant {
            use_scope: MediaUse::ProductPhotography,
            valid_from: 1100,
            term: RightsTerm::Perpetual,
            permission_ref: "new synthetic grant".into(),
        },
    )
    .await;
    let mut r = request("old-rights", &t);
    r.expected_revision = 1;
    assert!(matches!(
        record_image_review(&pool, &reviewer(), &FixedClock(1100), r).await,
        Err(ImageReviewError::Conflict)
    ));
    assert!(raw(&pool, "approved", 2, "reviewer").await.is_err());
    t.rights_revision = 2;
    record_image_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        request("new-rights", &t),
    )
    .await
    .expect("current rights");
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
    let mut r = request("old-bom", &t);
    r.expected_revision = 1;
    assert!(matches!(
        record_image_review(&pool, &reviewer(), &FixedClock(1100), r).await,
        Err(ImageReviewError::Conflict)
    ));
    specification(&pool, 1, 2, "spec-2").await;
    t.bom_revision = 2;
    t.specification_revision = 2;
    record_image_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        request("new-product", &t),
    )
    .await
    .expect("current product");
    let mut missing = t.clone();
    missing.asset_id = "missing".into();
    assert!(matches!(
        record_image_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            request("missing-image", &missing)
        )
        .await,
        Err(ImageReviewError::NotFound)
    ));
    let mut r = request("missing-product", &t);
    r.sku = "MISSING";
    assert!(matches!(
        record_image_review(&pool, &reviewer(), &FixedClock(1100), r).await,
        Err(ImageReviewError::NotFound)
    ));
    t.rights_revision = 3;
    assert!(matches!(
        record_image_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            request("unrecorded-rights", &t)
        )
        .await,
        Err(ImageReviewError::NotFound)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn future_and_inspiration_rights_cannot_approve_product_images(pool: PgPool) {
    let _case = media_seed(&pool).await;
    let mut t = target();
    for (revision, scope, start) in [
        (2, MediaUse::ProductPhotography, 1200),
        (3, MediaUse::Inspiration, 1100),
    ] {
        rights(
            &pool,
            revision - 1,
            &MediaRightsChange::Grant {
                use_scope: scope,
                valid_from: start,
                term: RightsTerm::Perpetual,
                permission_ref: "synthetic permission".into(),
            },
        )
        .await;
        t.rights_revision = revision;
        assert!(matches!(
            record_image_review(
                &pool,
                &reviewer(),
                &FixedClock(1100),
                request("wrong-use", &t)
            )
            .await,
            Err(ImageReviewError::Conflict)
        ));
    }
    let mut r = request("reject", &t);
    r.decision = ImageDecision::Rejected;
    record_image_review(&pool, &reviewer(), &FixedClock(1100), r)
        .await
        .expect("rejection still possible");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn every_retry_intent_field_is_bound_and_competing_reviewers_commit_once(pool: PgPool) {
    let _case = media_seed(&pool).await;
    let t = target();
    let reviewer_token = reviewer();
    let other_token = SessionToken::from_bytes([3; 32]);
    let (a, b) = tokio::join!(
        record_image_review(
            &pool,
            &reviewer_token,
            &FixedClock(1100),
            request("same", &t)
        ),
        record_image_review(
            &pool,
            &reviewer_token,
            &FixedClock(1100),
            request("same", &t)
        )
    );
    assert_ne!(a.expect("one").replayed, b.expect("other").replayed);
    for field in 0..7 {
        let mut r = request("same", &t);
        match field {
            0 => r.sku = "OTHER",
            1 => r.expected_revision = 1,
            2 => r.decision = ImageDecision::Rejected,
            3 => r.evidence_ref = "changed",
            4 => r.qualification_ref = "changed",
            5 => r.reason = "changed",
            _ => {}
        }
        let token = if field == 6 {
            SessionToken::from_bytes([3; 32])
        } else {
            reviewer()
        };
        assert!(
            matches!(
                record_image_review(&pool, &token, &FixedClock(1100), r).await,
                Err(ImageReviewError::Conflict)
            ),
            "field {field}"
        );
    }
    let mut changed = t.clone();
    changed.rights_revision = 2;
    assert!(matches!(
        record_image_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            request("same", &changed)
        )
        .await,
        Err(ImageReviewError::Conflict)
    ));
    let mut a = request("a", &t);
    a.expected_revision = 1;
    a.decision = ImageDecision::Rejected;
    let mut b = request("b", &t);
    b.expected_revision = 1;
    b.decision = ImageDecision::Rejected;
    let (a, b) = tokio::join!(
        record_image_review(&pool, &reviewer_token, &FixedClock(1100), a),
        record_image_review(&pool, &other_token, &FixedClock(1100), b)
    );
    assert_eq!([a.is_ok(), b.is_ok()].iter().filter(|v| **v).count(), 1);
    assert_eq!(count(&pool).await, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn final_authority_and_licence_clocks_guard_writes_reads_and_replays(pool: PgPool) {
    let _case = media_seed(&pool).await;
    let t = target();
    for (after, value) in [
        (1, Ok(2000)),
        (2, Ok(2000)),
        (2, Err(ContextError::ClockUnavailable)),
        (1, Ok(4600)),
    ] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            after,
            value,
        };
        assert!(
            record_image_review(&pool, &reviewer(), &clock, request("clock", &t))
                .await
                .is_err()
        );
        assert_eq!(count(&pool).await, 0);
    }
    record_image_review(&pool, &reviewer(), &FixedClock(1100), request("clock", &t))
        .await
        .expect("approve");
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        after: 1,
        value: Ok(4600),
    };
    assert!(
        record_image_review(&pool, &reviewer(), &clock, request("clock", &t))
            .await
            .is_err()
    );
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        after: 1,
        value: Err(ContextError::ClockUnavailable),
    };
    assert!(
        image_review_bundle(&pool, &reviewer(), &clock, "SET-FILLED", &t)
            .await
            .is_err()
    );
    // CPU byte validation reauthorizes after the data-read transaction.
    for after in [2, 3, 4] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            after,
            value: Ok(4600),
        };
        assert!(matches!(
            image_for_review(&pool, &reviewer(), &clock, "SET-FILLED", &t, 320).await,
            Err(ImageReviewError::Image(ImageError::Staff(
                StaffError::Denied
            )))
        ));
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn schema_is_immutable_independent_current_contiguous_and_merchant_only(pool: PgPool) {
    let _case = media_seed(&pool).await;
    assert!(raw(&pool, "approved", 1, "author").await.is_err());
    assert!(raw(&pool, "approved", 2, "reviewer").await.is_err());
    raw(&pool, "approved", 1, "reviewer")
        .await
        .expect("raw independent");
    for sql in [
        "UPDATE catalogue_image_reviews SET decision='rejected'",
        "DELETE FROM catalogue_image_reviews",
    ] {
        assert!(pool.execute(sql).await.is_err());
    }
    rights(&pool, 1, &MediaRightsChange::Revoke).await;
    assert!(raw(&pool, "approved", 2, "reviewer").await.is_err());
    raw(&pool, "rejected", 2, "reviewer")
        .await
        .expect("historical rejection");
    assert_eq!(count(&pool).await, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn real_source_lock_wait_expiry_cancellation_and_bounded_timeout_leave_no_effect(
    pool: PgPool,
) {
    let _case = media_seed(&pool).await;
    let t = target();
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute("UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku='SET-FILLED'")
        .await
        .expect("lock");
    let owned = pool.clone();
    let clock = std::sync::Arc::new(StepClock {
        calls: AtomicUsize::new(0),
        after: 1,
        value: Ok(4600),
    });
    let inside = clock.clone();
    let task = tokio::spawn(async move {
        record_image_review(
            &owned,
            &reviewer(),
            inside.as_ref(),
            request("wait", &target()),
        )
        .await
    });
    while clock.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    waiting_on(&pool, "UPDATE catalogue_variants%").await;
    blocker.rollback().await.expect("release");
    assert!(task.await.expect("task").is_err());
    assert_eq!(count(&pool).await, 0);
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute("UPDATE catalogue_media_serialization SET anchor=anchor WHERE asset_id='photo'")
        .await
        .expect("lock");
    let owned = pool.clone();
    let task = tokio::spawn(async move {
        record_image_review(
            &owned,
            &reviewer(),
            &FixedClock(1100),
            request("cancel", &target()),
        )
        .await
    });
    waiting_on(&pool, "SELECT asset_id FROM catalogue_media_serialization%").await;
    task.abort();
    assert!(
        task.await
            .expect_err("cancelled while waiting")
            .is_cancelled()
    );
    assert!(
        record_image_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            request("timeout", &t)
        )
        .await
        .is_err()
    );
    blocker.rollback().await.expect("release");
    assert_eq!(count(&pool).await, 0);
    assert!(
        record_image_review(&pool, &reviewer(), &FixedClock(1100), request("cancel", &t))
            .await
            .is_ok()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_repeatable_read_import_cannot_overlook_new_rights_or_specification(pool: PgPool) {
    let _case = media_seed(&pool).await;
    let mut stale = pool.begin().await.expect("snapshot");
    stale
        .execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .await
        .expect("isolation");
    stale
        .execute("SELECT * FROM catalogue_media_rights")
        .await
        .expect("capture");
    rights(&pool, 1, &MediaRightsChange::Revoke).await;
    let result=stale.execute("INSERT INTO catalogue_image_reviews(sku,asset_id,bom_revision,specification_revision,rights_revision,revision,effect_key,decision,evidence_ref,qualification_ref,actor,reason,evaluated_at) VALUES('SET-FILLED','photo',1,1,1,1,'stale','approved','evidence','qualification','reviewer','reason',1100)").await;
    assert_eq!(
        result
            .expect_err("retry stale rights")
            .as_database_error()
            .expect("database")
            .code()
            .as_deref(),
        Some("40001")
    );
    stale.rollback().await.expect("rollback");
    assert_eq!(count(&pool).await, 0);
}

#[sqlx::test(migrations = false)]
async fn upgrade_preserves_old_readers_candidates_and_private_bytes_without_approval(pool: PgPool) {
    MIGRATOR.run_to(14, &pool).await.expect("prior schema");
    let _case = media_seed(&pool).await;
    let old = commerce_infrastructure::catalogue(&pool, commerce_application::Locale::Nl)
        .await
        .expect("old reader");
    MIGRATOR.run(&pool).await.expect("upgrade");
    assert_eq!(
        commerce_infrastructure::catalogue(&pool, commerce_application::Locale::Nl)
            .await
            .expect("reader"),
        old
    );
    assert_eq!(count(&pool).await, 0);
    record_image_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        request("after-upgrade", &target()),
    )
    .await
    .expect("explicit review");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn source_or_permission_authors_cannot_review_even_when_separately_granted(pool: PgPool) {
    let _case = media_seed(&pool).await;
    // Distinct source actors exercise each independence constraint.
    for (n, id) in [
        (5, "bom-author"),
        (6, "spec-author"),
        (7, "register-author"),
        (8, "ingest-author"),
        (9, "rights-author"),
    ] {
        sqlx::query("INSERT INTO staff_accounts VALUES($1,true)")
            .bind(id)
            .execute(&pool)
            .await
            .expect("account");
        sqlx::query(
            "INSERT INTO staff_capabilities VALUES($1,'catalogue.manage'),($1,'catalogue.review')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .expect("grants");
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,4600,1100)").bind(SessionToken::from_bytes([n;32]).digest().as_slice()).bind(id).execute(&pool).await.expect("session");
    }
    // Historical imports represent already independently authored immutable sources.
    // Isolated test-only trigger suspension lets each source author vary separately.
    for (table, actor, n, statement) in [
        (
            "catalogue_definition_events",
            "bom-author",
            5,
            "ALTER TABLE catalogue_definition_events DISABLE TRIGGER USER; UPDATE catalogue_definition_events SET actor='bom-author' WHERE effect_key='variant'; ALTER TABLE catalogue_definition_events ENABLE TRIGGER USER",
        ),
        (
            "catalogue_specification_revisions",
            "spec-author",
            6,
            "ALTER TABLE catalogue_specification_revisions DISABLE TRIGGER USER; UPDATE catalogue_specification_revisions SET actor='spec-author'; ALTER TABLE catalogue_specification_revisions ENABLE TRIGGER USER",
        ),
        (
            "catalogue_media_assets",
            "register-author",
            7,
            "ALTER TABLE catalogue_media_assets DISABLE TRIGGER USER; UPDATE catalogue_media_assets SET actor='register-author'; ALTER TABLE catalogue_media_assets ENABLE TRIGGER USER",
        ),
        (
            "catalogue_image_originals",
            "ingest-author",
            8,
            "ALTER TABLE catalogue_image_originals DISABLE TRIGGER USER; UPDATE catalogue_image_originals SET actor='ingest-author'; ALTER TABLE catalogue_image_originals ENABLE TRIGGER USER",
        ),
        (
            "catalogue_media_rights",
            "rights-author",
            9,
            "ALTER TABLE catalogue_media_rights DISABLE TRIGGER USER; UPDATE catalogue_media_rights SET actor='rights-author'; ALTER TABLE catalogue_media_rights ENABLE TRIGGER USER",
        ),
    ] {
        pool.execute(statement)
            .await
            .expect("isolated imported author");
        assert!(
            matches!(
                record_image_review(
                    &pool,
                    &SessionToken::from_bytes([n; 32]),
                    &FixedClock(1100),
                    request("self", &target())
                )
                .await,
                Err(ImageReviewError::SelfReview)
            ),
            "{table}"
        );
        assert!(
            raw(&pool, "approved", 1, actor).await.is_err(),
            "raw {table}"
        );
    }
    assert_eq!(count(&pool).await, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn revocation_is_serialized_through_commit_and_then_denies_reads_and_receipts(pool: PgPool) {
    let _case = media_seed(&pool).await;
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute("LOCK TABLE catalogue_image_reviews IN SHARE MODE")
        .await
        .expect("block insert");
    let owned = pool.clone();
    let writer = tokio::spawn(async move {
        record_image_review(
            &owned,
            &reviewer(),
            &FixedClock(1100),
            request("authorized", &target()),
        )
        .await
    });
    waiting_on(&pool, "INSERT INTO catalogue_image_reviews%").await;
    let owned = pool.clone();
    let revoke = tokio::spawn(async move {
        owned.execute("DELETE FROM staff_capabilities WHERE staff_id='reviewer' AND capability='catalogue.review'").await
    });
    waiting_on(&pool, "DELETE FROM staff_capabilities%").await;
    assert!(!revoke.is_finished());
    blocker.rollback().await.expect("resume");
    writer.await.expect("task").expect("committed");
    revoke.await.expect("task").expect("revoked");
    assert!(matches!(
        record_image_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            request("authorized", &target())
        )
        .await,
        Err(ImageReviewError::Staff(StaffError::Denied))
    ));
    assert!(matches!(
        image_for_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            "SET-FILLED",
            &target(),
            320
        )
        .await,
        Err(ImageReviewError::Staff(StaffError::Denied))
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rights_cannot_expire_during_a_real_insert_wait_and_leave_approval(pool: PgPool) {
    struct MovingClock(std::sync::atomic::AtomicU64);
    impl Clock for MovingClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }
    let _case = media_seed(&pool).await;
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute("LOCK TABLE catalogue_image_reviews IN SHARE MODE")
        .await
        .expect("block insert");
    let clock = std::sync::Arc::new(MovingClock(std::sync::atomic::AtomicU64::new(1100)));
    let writer_clock = clock.clone();
    let owned = pool.clone();
    let writer = tokio::spawn(async move {
        record_image_review(
            &owned,
            &reviewer(),
            writer_clock.as_ref(),
            request("late", &target()),
        )
        .await
    });
    waiting_on(&pool, "INSERT INTO catalogue_image_reviews%").await;
    clock.0.store(2000, Ordering::SeqCst);
    blocker.rollback().await.expect("resume");
    assert!(matches!(
        writer.await.expect("task"),
        Err(ImageReviewError::Conflict)
    ));
    assert_eq!(count(&pool).await, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn corrupt_sources_rights_hashes_and_decisions_fail_private_reads_and_replays(pool: PgPool) {
    let _case = media_seed(&pool).await;
    let t = target();
    record_image_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        request("approved", &t),
    )
    .await
    .expect("review");
    for (corrupt, restore) in [
        (
            "ALTER TABLE catalogue_media_rights DISABLE TRIGGER USER; UPDATE catalogue_media_rights SET change='{}'",
            "UPDATE catalogue_media_rights SET change='{\"action\":\"grant\",\"use_scope\":\"product-photography\",\"valid_from\":1100,\"term\":{\"kind\":\"until\",\"expires_at\":2000},\"permission_ref\":\"synthetic permission\"}'; ALTER TABLE catalogue_media_rights ENABLE TRIGGER USER",
        ),
        (
            "ALTER TABLE catalogue_definition_events DISABLE TRIGGER USER; UPDATE catalogue_definition_events SET payload=payload || '{\"unknown\":true}' WHERE effect_key='variant'",
            "UPDATE catalogue_definition_events SET payload=payload-'unknown' WHERE effect_key='variant'; ALTER TABLE catalogue_definition_events ENABLE TRIGGER USER",
        ),
        (
            "ALTER TABLE catalogue_image_reviews DISABLE TRIGGER USER; UPDATE catalogue_image_reviews SET actor='author'",
            "UPDATE catalogue_image_reviews SET actor='reviewer'; ALTER TABLE catalogue_image_reviews ENABLE TRIGGER USER",
        ),
    ] {
        pool.execute(corrupt).await.expect("corrupt isolated DB");
        assert!(
            image_review_bundle(&pool, &reviewer(), &FixedClock(1100), "SET-FILLED", &t)
                .await
                .is_err()
        );
        assert!(
            record_image_review(
                &pool,
                &reviewer(),
                &FixedClock(1100),
                request("approved", &t)
            )
            .await
            .is_err()
        );
        pool.execute(restore).await.expect("restore guards");
    }
    pool.execute("ALTER TABLE catalogue_image_renditions DISABLE TRIGGER USER; ALTER TABLE catalogue_image_renditions DROP CONSTRAINT catalogue_image_renditions_check2; UPDATE catalogue_image_renditions SET sha256=decode(repeat('00',32),'hex')").await.expect("hash corruption");
    assert!(matches!(
        image_review_bundle(&pool, &reviewer(), &FixedClock(1100), "SET-FILLED", &t).await,
        Err(ImageReviewError::InvalidStoredData)
    ));
    pool.execute("UPDATE catalogue_image_renditions SET sha256=sha256(bytes); ALTER TABLE catalogue_image_renditions ADD CONSTRAINT catalogue_image_renditions_check2 CHECK(sha256=sha256(bytes)); ALTER TABLE catalogue_image_renditions ENABLE TRIGGER USER").await.expect("restore hash");
    assert!(
        record_image_review(
            &pool,
            &reviewer(),
            &FixedClock(1100),
            request("approved", &t)
        )
        .await
        .expect("restored")
        .replayed
    );
}

async fn waiting_on(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(800),async {loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(pattern).fetch_one(pool).await.expect("wait oracle");
        if waiting {break;}tokio::task::yield_now().await;
    }}).await.expect("actual database lock wait");
}
