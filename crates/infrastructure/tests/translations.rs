use commerce_application::{
    Locale,
    request_context::{Clock, ContextError},
};
use commerce_domain::translations::{TranslationDecision, TranslationDraft};
use commerce_infrastructure::{
    MIGRATOR,
    specifications::{SpecificationRequest, record_specification},
    staff_access::{SessionToken, StaffError},
    translations::*,
};
use sqlx::{Executor, PgPool};
include!("../../../tests/fixtures/specification.rs");

#[sqlx::test(migrator = "MIGRATOR")]
async fn historical_approvals_can_be_withdrawn_after_source_and_specification_changes(
    pool: PgPool,
) {
    seed(&pool).await;
    let author = SessionToken::from_bytes([1; 32]);
    let reader = SessionToken::from_bytes([2; 32]);
    let clock = FixedClock(1100);
    let content = copy();
    record_translation(&pool, &author, &clock, request(&content, Locale::Nl, "nl1"))
        .await
        .expect("source");
    record_translation(&pool, &author, &clock, request(&content, Locale::De, "de1"))
        .await
        .expect("target");
    record_review(&pool, &reader, &clock, review(Locale::Nl, "nl-approval"))
        .await
        .expect("old source approval");
    record_review(&pool, &reader, &clock, review(Locale::De, "de-approval"))
        .await
        .expect("old target approval");
    let mut newer = request(&content, Locale::Nl, "nl2");
    newer.expected_revision = 1;
    record_translation(&pool, &author, &clock, newer)
        .await
        .expect("newer source draft");
    let mut facts = draft(true);
    facts.footprint.width_mm = 1300;
    record_specification(
        &pool,
        &author,
        &clock,
        SpecificationRequest {
            sku: "DEMO-FILLED",
            bom_revision: 1,
            expected_revision: 1,
            effect_key: "facts2",
            draft: &facts,
            reason: "isolated changed facts",
            confirmed: true,
        },
    )
    .await
    .expect("new specification");
    let mut source = request(&content, Locale::Nl, "nl3");
    source.expected_revision = 2;
    source.specification_revision = 2;
    record_translation(&pool, &author, &clock, source)
        .await
        .expect("new fact source");
    let mut target = request(&content, Locale::De, "de2");
    target.expected_revision = 1;
    target.specification_revision = 2;
    target.source_revision = Some(3);
    record_translation(&pool, &author, &clock, target)
        .await
        .expect("new target draft");
    for (locale, old_key, withdraw_key) in [
        (Locale::Nl, "nl-approval", "withdraw-old-nl"),
        (Locale::De, "de-approval", "withdraw-old-de"),
    ] {
        let mut withdrawal = review(locale, withdraw_key);
        withdrawal.expected_review_revision = 1;
        withdrawal.decision = TranslationDecision::Rejected;
        let result = record_review(&pool, &reader, &clock, withdrawal)
            .await
            .expect("withdraw historical approval");
        assert_eq!((result.revision, result.replayed), (2, false));
        assert!(
            record_review(&pool, &reader, &clock, review(locale, old_key))
                .await
                .expect("old immutable receipt")
                .replayed
        );
        let history = translation(&pool, &author, &clock, "DEMO-FILLED", locale, 1)
            .await
            .expect("historical read")
            .expect("copy");
        assert_eq!(
            history.review.expect("latest historical decision").decision,
            TranslationDecision::Rejected
        );
        let mut approval = review(locale, "stale-new-approval");
        approval.expected_review_revision = 2;
        assert!(matches!(
            record_review(&pool, &reader, &clock, approval).await,
            Err(TranslationError::Conflict)
        ));
        let mut stale = review(locale, "stale-rejection-version");
        stale.expected_review_revision = 1;
        stale.decision = TranslationDecision::Rejected;
        assert!(matches!(
            record_review(&pool, &reader, &clock, stale).await,
            Err(TranslationError::Conflict)
        ));
    }
    assert_eq!(counts(&pool).await, (5, 4));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bounded_effect_storage_failure_rolls_back_and_allows_an_exact_retry(pool: PgPool) {
    seed(&pool).await;
    let author = SessionToken::from_bytes([1; 32]);
    let content = copy();
    let clock = FixedClock(1100);
    record_translation(
        &pool,
        &author,
        &clock,
        request(&content, Locale::Nl, "source"),
    )
    .await
    .expect("source");
    let mut blocker = pool.begin().await.expect("bounded effect blocker");
    blocker.execute("SELECT pg_advisory_xact_lock(hashtextextended('catalogue-translation-draft:timeout',0))").await.expect("hold effect");
    let mut r = request(&content, Locale::Nl, "timeout");
    r.expected_revision = 1;
    let error = record_translation(&pool, &author, &clock, r)
        .await
        .expect_err("bounded dependency failure");
    assert!(
        matches!(error,TranslationError::Database(ref e) if e.as_database_error().and_then(|e|e.code()).as_deref()==Some("55P03"))
    );
    assert_eq!(counts(&pool).await, (1, 0));
    blocker.rollback().await.expect("resume storage");
    let mut r = request(&content, Locale::Nl, "timeout");
    r.expected_revision = 1;
    let result = record_translation(&pool, &author, &clock, r)
        .await
        .expect("recover same intent");
    assert_eq!((result.revision, result.replayed), (2, false));
    assert_eq!(counts(&pool).await, (2, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_repeatable_read_imports_cannot_bless_old_sources_or_copy(pool: PgPool) {
    seed(&pool).await;
    let author = SessionToken::from_bytes([1; 32]);
    let content = copy();
    let clock = FixedClock(1100);
    record_translation(&pool, &author, &clock, request(&content, Locale::Nl, "nl1"))
        .await
        .expect("source");
    for (expected, review_write) in [(1, false), (2, true)] {
        let mut stale = pool.begin().await.expect("stale importer");
        stale
            .execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .await
            .expect("repeatable read");
        let _: i64 = sqlx::query_scalar("SELECT count(*) FROM catalogue_translation_revisions")
            .fetch_one(&mut *stale)
            .await
            .expect("capture old snapshot");
        let key = format!("nl{}", expected + 1);
        let mut next = request(&content, Locale::Nl, &key);
        next.expected_revision = expected;
        record_translation(&pool, &author, &clock, next)
            .await
            .expect("concurrent source edit");
        let error = if review_write {
            sqlx::query("INSERT INTO catalogue_translation_reviews(sku,locale,translation_revision,revision,effect_key,decision,evidence_ref,qualification_ref,actor,reason) VALUES('DEMO-FILLED','nl',2,1,'stale-review','approved','fixture','fixture','reader','fixture')").execute(&mut *stale).await.expect_err("cannot review old snapshot")
        } else {
            sqlx::query("INSERT INTO catalogue_translation_revisions(sku,locale,revision,specification_revision,source_revision,effect_key,payload,actor,reason) VALUES('DEMO-FILLED','de',1,1,1,'stale-copy',$1::jsonb,'author','fixture')").bind(serde_json::to_string(&content).expect("fixture")).execute(&mut *stale).await.expect_err("cannot translate old source")
        };
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("40001")
        );
        stale.rollback().await.expect("discard stale import");
    }
    assert_eq!(counts(&pool).await, (3, 0));
    let mut fresh = review(Locale::Nl, "fresh-review");
    fresh.translation_revision = 3;
    record_review(&pool, &SessionToken::from_bytes([2; 32]), &clock, fresh)
        .await
        .expect("fresh review works");
    assert_eq!(counts(&pool).await, (3, 1));
}

struct FixedClock(u64);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
fn copy() -> TranslationDraft {
    TranslationDraft {
        title: "isolated fixture title".into(),
        description: "invented isolated description; never merchant copy".into(),
        materials: "invented material description".into(),
        care: "invented care instructions".into(),
        safety: "invented safety text; no actual safety evidence".into(),
        assembly: "invented assembly text".into(),
    }
}
fn request<'a>(
    content: &'a TranslationDraft,
    locale: Locale,
    key: &'a str,
) -> TranslationRequest<'a> {
    TranslationRequest {
        sku: "DEMO-FILLED",
        locale,
        specification_revision: 1,
        source_revision: if locale == Locale::Nl { None } else { Some(1) },
        expected_revision: 0,
        effect_key: key,
        draft: content,
        reason: "isolated copy fixture",
        confirmed: true,
    }
}
fn review(locale: Locale, key: &str) -> TranslationReviewRequest<'_> {
    TranslationReviewRequest {
        sku: "DEMO-FILLED",
        locale,
        translation_revision: 1,
        expected_review_revision: 0,
        effect_key: key,
        decision: TranslationDecision::Approved,
        evidence_ref: "fixture-only-review-not-human-acceptance",
        qualification_ref: "fixture-only-qualification-not-a-real-reader",
        reason: "isolated review fixture",
        confirmed: true,
    }
}
async fn seed(pool: &PgPool) {
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("isolated catalogue");
    pool.execute("INSERT INTO catalogue_translations(sku,locale,title,description,care) VALUES('DEMO-FILLED','nl','legacy fixture title','legacy fixture description','legacy fixture care'),('DEMO-FILLED','de','legacy German fixture','legacy fixture description','legacy fixture care')").await.expect("existing preview copy");
    pool.execute("INSERT INTO staff_accounts VALUES ('author',true),('reader',true),('warehouse',true),('other-author',true),('other-reader',true);
        INSERT INTO staff_capabilities VALUES ('author','catalogue.manage'),('reader','catalogue.review'),('warehouse','warehouse.manage'),('other-author','catalogue.manage'),('other-reader','catalogue.review')").await.expect("test-only authority");
    for (number, actor) in [
        (1, "author"),
        (2, "reader"),
        (3, "warehouse"),
        (4, "other-author"),
        (5, "other-reader"),
    ] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,4600,1100)").bind(SessionToken::from_bytes([number;32]).digest().as_slice()).bind(actor).execute(pool).await.expect("test-only session");
    }
    record_specification(
        pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        SpecificationRequest {
            sku: "DEMO-FILLED",
            bom_revision: 1,
            expected_revision: 0,
            effect_key: "fixture-specification",
            draft: &draft(true),
            reason: "invented isolated source",
            confirmed: true,
        },
    )
    .await
    .expect("source specification");
}
async fn counts(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM catalogue_translation_revisions),(SELECT count(*) FROM catalogue_translation_reviews)").fetch_one(pool).await.expect("effect oracle")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn seven_locales_have_explicit_source_bound_copy_and_independent_review_history(
    pool: PgPool,
) {
    seed(&pool).await;
    let author = SessionToken::from_bytes([1; 32]);
    let reader = SessionToken::from_bytes([2; 32]);
    let clock = FixedClock(1100);
    let content = copy();
    let before: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT sku,locale,title,description,care FROM catalogue_translations ORDER BY sku,locale",
    )
    .fetch_all(&pool)
    .await
    .expect("preview oracle");
    for (locale, key) in [
        (Locale::Nl, "nl"),
        (Locale::De, "de"),
        (Locale::Fr, "fr"),
        (Locale::En, "en"),
        (Locale::Ar, "ar"),
        (Locale::Tr, "tr"),
        (Locale::Kmr, "kmr"),
    ] {
        let created = record_translation(&pool, &author, &clock, request(&content, locale, key))
            .await
            .expect("draft");
        assert_eq!((created.revision, created.replayed), (1, false));
        assert!(
            record_translation(&pool, &author, &clock, request(&content, locale, key))
                .await
                .expect("exact replay")
                .replayed
        );
        let before_review =
            translation_for_review(&pool, &reader, &clock, "DEMO-FILLED", locale, 1)
                .await
                .expect("review read")
                .expect("copy");
        assert_eq!(before_review.review, None);
        assert_eq!(before_review.draft, content);
        assert_eq!(before_review.actor, "author");
        let result = record_review(&pool, &reader, &clock, review(locale, key))
            .await
            .expect("independent review");
        assert_eq!((result.revision, result.replayed), (1, false));
        assert!(
            record_review(&pool, &reader, &clock, review(locale, key))
                .await
                .expect("review replay")
                .replayed
        );
        let current = translation(&pool, &author, &clock, "DEMO-FILLED", locale, 1)
            .await
            .expect("author read")
            .expect("copy");
        let accepted = current.review.expect("attestation");
        assert_eq!(accepted.decision, TranslationDecision::Approved);
        assert_eq!(accepted.actor, "reader");
    }
    let mut rejected = review(Locale::Nl, "withdraw-review");
    rejected.expected_review_revision = 1;
    rejected.decision = TranslationDecision::Rejected;
    assert_eq!(
        record_review(&pool, &reader, &clock, rejected)
            .await
            .expect("withdrawal")
            .revision,
        2
    );
    assert!(
        record_review(&pool, &reader, &clock, review(Locale::Nl, "nl"))
            .await
            .expect("historical replay")
            .replayed
    );
    assert_eq!(
        translation(&pool, &author, &clock, "DEMO-FILLED", Locale::Nl, 1)
            .await
            .expect("current history")
            .expect("copy")
            .review
            .expect("latest decision")
            .decision,
        TranslationDecision::Rejected
    );
    let after: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT sku,locale,title,description,care FROM catalogue_translations ORDER BY sku,locale",
    )
    .fetch_all(&pool)
    .await
    .expect("preview after");
    assert_eq!(before, after);
    assert_eq!(counts(&pool).await, (7, 8));
    assert!(
        translation(&pool, &author, &clock, "DEMO-FILLED", Locale::Nl, 99)
            .await
            .expect("missing")
            .is_none()
    );
}

#[test]
fn content_cannot_supply_a_review_flag_or_omit_required_translated_fields() {
    let mut payload = serde_json::to_value(copy()).expect("copy");
    payload["reviewed"] = serde_json::json!(true);
    assert!(serde_json::from_value::<TranslationDraft>(payload).is_err());
    let mut missing = serde_json::to_value(copy()).expect("copy");
    missing.as_object_mut().expect("object").remove("materials");
    assert!(serde_json::from_value::<TranslationDraft>(missing).is_err());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authoring_review_and_warehouse_grants_are_distinct_and_self_review_is_rejected(
    pool: PgPool,
) {
    seed(&pool).await;
    let author = SessionToken::from_bytes([1; 32]);
    let reader = SessionToken::from_bytes([2; 32]);
    let clock = FixedClock(1100);
    let content = copy();
    for number in [0, 2, 3, 5] {
        assert!(matches!(
            record_translation(
                &pool,
                &SessionToken::from_bytes([number; 32]),
                &clock,
                request(&content, Locale::Nl, "denied")
            )
            .await,
            Err(TranslationError::Staff(StaffError::Denied))
        ));
    }
    for number in [0, 1, 3, 4] {
        assert!(matches!(
            record_review(
                &pool,
                &SessionToken::from_bytes([number; 32]),
                &clock,
                review(Locale::Nl, "denied")
            )
            .await,
            Err(TranslationError::Staff(StaffError::Denied))
        ));
    }
    assert!(matches!(
        translation(&pool, &reader, &clock, "DEMO-FILLED", Locale::Nl, 1).await,
        Err(TranslationError::Staff(StaffError::Denied))
    ));
    assert!(matches!(
        translation_for_review(&pool, &author, &clock, "DEMO-FILLED", Locale::Nl, 1).await,
        Err(TranslationError::Staff(StaffError::Denied))
    ));
    assert!(
        translation_for_review(&pool, &reader, &clock, "DEMO-FILLED", Locale::Nl, 1)
            .await
            .expect("review read without author grant")
            .is_none()
    );
    for (sku, revision) in [("", 1), ("DEMO-FILLED", 0)] {
        assert!(matches!(
            translation(&pool, &author, &clock, sku, Locale::Nl, revision).await,
            Err(TranslationError::InvalidRequest)
        ));
        assert!(matches!(
            translation_for_review(&pool, &reader, &clock, sku, Locale::Nl, revision).await,
            Err(TranslationError::InvalidRequest)
        ));
    }
    for case in 0..13 {
        let mut r = request(&content, Locale::Nl, "invalid");
        let long = "x".repeat(501);
        let sku = "X".repeat(66);
        match case {
            0 => r.confirmed = false,
            1 => r.sku = "",
            2 => r.sku = &sku,
            3 => r.specification_revision = 0,
            4 => r.source_revision = Some(1),
            5 => r.expected_revision = -1,
            6 => r.expected_revision = i32::MAX,
            7 => r.effect_key = "/",
            8 => r.reason = " ",
            9 => r.reason = &long,
            10 => {
                r.locale = Locale::De;
                r.source_revision = None;
            }
            11 => {
                r.locale = Locale::De;
                r.source_revision = Some(0);
            }
            12 => r.reason = "nul\0reason",
            _ => unreachable!(),
        }
        assert!(
            matches!(
                record_translation(&pool, &author, &clock, r).await,
                Err(TranslationError::InvalidRequest)
            ),
            "draft envelope {case}"
        );
    }
    let mut invalid_copy = content.clone();
    invalid_copy.care = " ".into();
    assert!(matches!(
        record_translation(
            &pool,
            &author,
            &clock,
            request(&invalid_copy, Locale::Nl, "blank-copy")
        )
        .await,
        Err(TranslationError::InvalidRequest)
    ));
    for case in 0..10 {
        let mut r = review(Locale::Nl, "invalid");
        let long = "x".repeat(501);
        match case {
            0 => r.confirmed = false,
            1 => r.sku = "",
            2 => r.translation_revision = 0,
            3 => r.expected_review_revision = -1,
            4 => r.expected_review_revision = i32::MAX,
            5 => r.effect_key = "/",
            6 => r.evidence_ref = " ",
            7 => r.qualification_ref = " ",
            8 => r.reason = " ",
            9 => r.qualification_ref = &long,
            _ => unreachable!(),
        }
        assert!(
            matches!(
                record_review(&pool, &reader, &clock, r).await,
                Err(TranslationError::InvalidRequest)
            ),
            "review envelope {case}"
        );
    }
    assert!(matches!(
        record_review(&pool, &reader, &clock, review(Locale::Nl, "missing")).await,
        Err(TranslationError::NotFound)
    ));
    let mut missing = request(&content, Locale::Nl, "missing-sku");
    missing.sku = "DEMO-MISSING";
    assert!(matches!(
        record_translation(&pool, &author, &clock, missing).await,
        Err(TranslationError::NotFound)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    record_translation(
        &pool,
        &author,
        &clock,
        request(&content, Locale::Nl, "copy"),
    )
    .await
    .expect("draft");
    pool.execute("INSERT INTO staff_capabilities VALUES('author','catalogue.review')")
        .await
        .expect("explicit second grant");
    assert!(matches!(
        record_review(&pool, &author, &clock, review(Locale::Nl, "self-review")).await,
        Err(TranslationError::SelfReview)
    ));
    let raw=pool.execute("INSERT INTO catalogue_translation_reviews(sku,locale,translation_revision,revision,effect_key,decision,evidence_ref,qualification_ref,actor,reason) VALUES('DEMO-FILLED','nl',1,1,'raw-self','approved','fixture','fixture','author','fixture')").await.expect_err("schema also rejects self review");
    assert_eq!(
        raw.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    assert_eq!(counts(&pool).await, (1, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_source_copy_or_specification_cannot_receive_stale_translation_approval(
    pool: PgPool,
) {
    seed(&pool).await;
    let author = SessionToken::from_bytes([1; 32]);
    let reader = SessionToken::from_bytes([2; 32]);
    let clock = FixedClock(1100);
    let content = copy();
    assert!(matches!(
        record_translation(
            &pool,
            &author,
            &clock,
            request(&content, Locale::De, "without-source")
        )
        .await,
        Err(TranslationError::Conflict)
    ));
    record_translation(&pool, &author, &clock, request(&content, Locale::Nl, "nl1"))
        .await
        .expect("Dutch source");
    record_translation(&pool, &author, &clock, request(&content, Locale::De, "de1"))
        .await
        .expect("German draft");
    let mut changed = content.clone();
    changed.description = "changed isolated source".into();
    let mut next = request(&changed, Locale::Nl, "nl2");
    next.expected_revision = 1;
    record_translation(&pool, &author, &clock, next)
        .await
        .expect("next Dutch revision");
    assert!(matches!(
        record_review(&pool, &reader, &clock, review(Locale::Nl, "old-dutch")).await,
        Err(TranslationError::Conflict)
    ));
    assert!(matches!(
        record_review(&pool, &reader, &clock, review(Locale::De, "old-source")).await,
        Err(TranslationError::Conflict)
    ));
    assert!(matches!(
        record_translation(
            &pool,
            &author,
            &clock,
            request(&content, Locale::Fr, "old-source")
        )
        .await,
        Err(TranslationError::Conflict)
    ));
    let mut de = request(&content, Locale::De, "de2");
    de.expected_revision = 1;
    de.source_revision = Some(2);
    record_translation(&pool, &author, &clock, de)
        .await
        .expect("current Dutch source");
    let mut accept = review(Locale::De, "de-review2");
    accept.translation_revision = 2;
    record_review(&pool, &reader, &clock, accept)
        .await
        .expect("current draft review");
    assert!(
        record_translation(&pool, &author, &clock, request(&content, Locale::De, "de1"))
            .await
            .expect("historical draft replay")
            .replayed
    );
    let mut specification = draft(true);
    specification.footprint.width_mm = 1200;
    record_specification(
        &pool,
        &author,
        &clock,
        SpecificationRequest {
            sku: "DEMO-FILLED",
            bom_revision: 1,
            expected_revision: 1,
            effect_key: "new-facts",
            draft: &specification,
            reason: "isolated source change",
            confirmed: true,
        },
    )
    .await
    .expect("new immutable facts");
    let mut stale = review(Locale::De, "stale-facts");
    stale.translation_revision = 2;
    stale.expected_review_revision = 1;
    assert!(matches!(
        record_review(&pool, &reader, &clock, stale).await,
        Err(TranslationError::Conflict)
    ));
    let mut wrong_source = request(&content, Locale::Fr, "wrong-spec-source");
    wrong_source.specification_revision = 2;
    wrong_source.source_revision = Some(2);
    assert!(matches!(
        record_translation(&pool, &author, &clock, wrong_source).await,
        Err(TranslationError::Conflict)
    ));
    let mut nl3 = request(&content, Locale::Nl, "nl3");
    nl3.specification_revision = 2;
    nl3.expected_revision = 2;
    record_translation(&pool, &author, &clock, nl3)
        .await
        .expect("new facts need new Dutch copy");
    let mut fr = request(&content, Locale::Fr, "fr-current");
    fr.specification_revision = 2;
    fr.source_revision = Some(3);
    record_translation(&pool, &author, &clock, fr)
        .await
        .expect("fresh translation");
    assert_eq!(counts(&pool).await, (6, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_authors_readers_and_effect_replays_preserve_complete_intent(pool: PgPool) {
    seed(&pool).await;
    let author = SessionToken::from_bytes([1; 32]);
    let other_author = SessionToken::from_bytes([4; 32]);
    let reader = SessionToken::from_bytes([2; 32]);
    let other_reader = SessionToken::from_bytes([5; 32]);
    let clock = FixedClock(1100);
    let content = copy();
    record_translation(&pool, &author, &clock, request(&content, Locale::Nl, "nl"))
        .await
        .expect("source");
    let (a, b) = tokio::join!(
        record_translation(&pool, &author, &clock, request(&content, Locale::De, "de")),
        record_translation(&pool, &author, &clock, request(&content, Locale::De, "de"))
    );
    assert_ne!(
        a.expect("write/replay").replayed,
        b.expect("write/replay").replayed
    );
    let (a, b) = tokio::join!(
        record_review(&pool, &reader, &clock, review(Locale::De, "review")),
        record_review(&pool, &reader, &clock, review(Locale::De, "review"))
    );
    assert_ne!(
        a.expect("review/replay").replayed,
        b.expect("review/replay").replayed
    );
    let mut changed = content.clone();
    changed.title = "changed isolated copy".into();
    for case in 0..8 {
        let mut r = request(&content, Locale::De, "de");
        let actor = if case == 6 { &other_author } else { &author };
        match case {
            0 => r.sku = "DEMO-COVERS",
            1 => r.locale = Locale::Fr,
            2 => r.specification_revision = 2,
            3 => r.source_revision = Some(2),
            4 => r.expected_revision = 1,
            5 => r.draft = &changed,
            6 => {}
            7 => r.reason = "different intent",
            _ => unreachable!(),
        }
        assert!(
            matches!(
                record_translation(&pool, actor, &clock, r).await,
                Err(TranslationError::Conflict)
            ),
            "draft identity {case}"
        );
    }
    for case in 0..9 {
        let mut r = review(Locale::De, "review");
        let actor = if case == 8 { &other_reader } else { &reader };
        match case {
            0 => r.sku = "DEMO-COVERS",
            1 => r.locale = Locale::Fr,
            2 => r.translation_revision = 2,
            3 => r.expected_review_revision = 1,
            4 => r.decision = TranslationDecision::Rejected,
            5 => r.evidence_ref = "other evidence",
            6 => r.qualification_ref = "other qualification",
            7 => r.reason = "other reason",
            8 => {}
            _ => unreachable!(),
        }
        assert!(
            matches!(
                record_review(&pool, actor, &clock, r).await,
                Err(TranslationError::Conflict)
            ),
            "review identity {case}"
        );
    }
    let mut left = request(&changed, Locale::De, "editor-left");
    left.expected_revision = 1;
    let mut right = request(&content, Locale::De, "editor-right");
    right.expected_revision = 1;
    let (left, right) = tokio::join!(
        record_translation(&pool, &author, &clock, left),
        record_translation(&pool, &other_author, &clock, right)
    );
    assert!(matches!(
        (&left, &right),
        (Ok(_), Err(TranslationError::Conflict)) | (Err(TranslationError::Conflict), Ok(_))
    ));
    let mut left = review(Locale::De, "reader-left");
    left.translation_revision = 2;
    let mut right = review(Locale::De, "reader-right");
    right.translation_revision = 2;
    let (left, right) = tokio::join!(
        record_review(&pool, &reader, &clock, left),
        record_review(&pool, &other_reader, &clock, right)
    );
    assert!(matches!(
        (&left, &right),
        (Ok(_), Err(TranslationError::Conflict)) | (Err(TranslationError::Conflict), Ok(_))
    ));
    assert!(matches!(
        record_review(&pool, &reader, &clock, review(Locale::De, "stale-target")).await,
        Err(TranslationError::Conflict)
    ));
    let mut stale_review = review(Locale::De, "stale-review-version");
    stale_review.translation_revision = 2;
    assert!(matches!(
        record_review(&pool, &reader, &clock, stale_review).await,
        Err(TranslationError::Conflict)
    ));
    assert_eq!(counts(&pool).await, (3, 2));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn final_clock_failures_roll_back_copy_and_reviews_and_hide_private_reads(pool: PgPool) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct StepClock {
        calls: AtomicUsize,
        unavailable: bool,
        overflow: bool,
    }
    impl Clock for StepClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(1100)
            } else if self.unavailable {
                Err(ContextError::ClockUnavailable)
            } else if self.overflow {
                Ok(u64::MAX)
            } else {
                Ok(4600)
            }
        }
    }
    seed(&pool).await;
    let author = SessionToken::from_bytes([1; 32]);
    let reader = SessionToken::from_bytes([2; 32]);
    let content = copy();
    for (unavailable, overflow) in [(false, false), (true, false), (false, true)] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            unavailable,
            overflow,
        };
        assert!(matches!(
            record_translation(
                &pool,
                &author,
                &clock,
                request(&content, Locale::Nl, "expired-copy")
            )
            .await,
            Err(TranslationError::Staff(
                StaffError::Denied | StaffError::ClockUnavailable
            ))
        ));
        assert_eq!(counts(&pool).await, (0, 0));
    }
    record_translation(
        &pool,
        &author,
        &FixedClock(1100),
        request(&content, Locale::Nl, "copy"),
    )
    .await
    .expect("valid copy");
    for (unavailable, overflow) in [(false, false), (true, false), (false, true)] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            unavailable,
            overflow,
        };
        assert!(matches!(
            record_review(&pool, &reader, &clock, review(Locale::Nl, "expired-review")).await,
            Err(TranslationError::Staff(
                StaffError::Denied | StaffError::ClockUnavailable
            ))
        ));
        assert_eq!(counts(&pool).await, (1, 0));
        for revision in [1, 99] {
            let clock = StepClock {
                calls: AtomicUsize::new(0),
                unavailable,
                overflow,
            };
            assert!(matches!(
                translation(&pool, &author, &clock, "DEMO-FILLED", Locale::Nl, revision).await,
                Err(TranslationError::Staff(
                    StaffError::Denied | StaffError::ClockUnavailable
                ))
            ));
            let clock = StepClock {
                calls: AtomicUsize::new(0),
                unavailable,
                overflow,
            };
            assert!(matches!(
                translation_for_review(&pool, &reader, &clock, "DEMO-FILLED", Locale::Nl, revision)
                    .await,
                Err(TranslationError::Staff(
                    StaffError::Denied | StaffError::ClockUnavailable
                ))
            ));
        }
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            unavailable,
            overflow,
        };
        assert!(matches!(
            record_translation(
                &pool,
                &author,
                &clock,
                request(&content, Locale::Nl, "copy")
            )
            .await,
            Err(TranslationError::Staff(
                StaffError::Denied | StaffError::ClockUnavailable
            ))
        ));
    }
    record_review(
        &pool,
        &reader,
        &FixedClock(1100),
        review(Locale::Nl, "review"),
    )
    .await
    .expect("valid review");
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        unavailable: false,
        overflow: false,
    };
    assert!(matches!(
        record_review(&pool, &reader, &clock, review(Locale::Nl, "review")).await,
        Err(TranslationError::Staff(StaffError::Denied))
    ));
    assert_eq!(counts(&pool).await, (1, 1));
}

async fn waiting_on(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(800),async {loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(pattern).fetch_one(pool).await.expect("wait oracle");
        if waiting {break;} tokio::task::yield_now().await;
    }}).await.expect("operation reached actual database lock");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn source_and_effect_lock_waits_do_not_extend_recent_authority(pool: PgPool) {
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
    seed(&pool).await;
    let content = copy();
    let author = SessionToken::from_bytes([1; 32]);
    record_translation(
        &pool,
        &author,
        &FixedClock(1100),
        request(&content, Locale::Nl, "source"),
    )
    .await
    .expect("source copy");
    for review_write in [false, true] {
        let mut blocker = pool.begin().await.expect("source blocker");
        blocker
            .execute(
                "UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku='DEMO-FILLED'",
            )
            .await
            .expect("hold variant");
        let clock = Arc::new(MovingClock(AtomicU64::new(1100)));
        let writer_clock = clock.clone();
        let writer_pool = pool.clone();
        let writer = tokio::spawn(async move {
            if review_write {
                record_review(
                    &writer_pool,
                    &SessionToken::from_bytes([2; 32]),
                    writer_clock.as_ref(),
                    review(Locale::Nl, "wait-review"),
                )
                .await
            } else {
                record_translation(
                    &writer_pool,
                    &SessionToken::from_bytes([1; 32]),
                    writer_clock.as_ref(),
                    request(&copy(), Locale::De, "wait-draft"),
                )
                .await
            }
        });
        waiting_on(&pool, "UPDATE catalogue_variants%RETURNING sku").await;
        clock.0.store(4600, Ordering::SeqCst);
        blocker.rollback().await.expect("resume expired command");
        assert!(matches!(
            writer.await.expect("task"),
            Err(TranslationError::Staff(StaffError::Denied))
        ));
        assert_eq!(counts(&pool).await, (1, 0));
    }
    let mut blocker = pool.begin().await.expect("effect blocker");
    blocker.execute("SELECT pg_advisory_xact_lock(hashtextextended('catalogue-translation-draft:source',0))").await.expect("hold replay effect");
    let clock = Arc::new(MovingClock(AtomicU64::new(1100)));
    let replay_clock = clock.clone();
    let replay_pool = pool.clone();
    let replay = tokio::spawn(async move {
        record_translation(
            &replay_pool,
            &SessionToken::from_bytes([1; 32]),
            replay_clock.as_ref(),
            request(&copy(), Locale::Nl, "source"),
        )
        .await
    });
    waiting_on(&pool, "SELECT pg_advisory_xact_lock%").await;
    clock.0.store(4600, Ordering::SeqCst);
    blocker.rollback().await.expect("resume expired replay");
    assert!(matches!(
        replay.await.expect("task"),
        Err(TranslationError::Staff(StaffError::Denied))
    ));
    assert_eq!(counts(&pool).await, (1, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn review_grant_revocation_waits_for_commit_then_denies_every_later_effect(pool: PgPool) {
    seed(&pool).await;
    record_translation(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        request(&copy(), Locale::Nl, "copy"),
    )
    .await
    .expect("copy");
    let mut blocker = pool.begin().await.expect("effect blocker");
    blocker.execute("SELECT pg_advisory_xact_lock(hashtextextended('catalogue-translation-review:paused',0))").await.expect("pause review");
    let writer_pool = pool.clone();
    let writer = tokio::spawn(async move {
        record_review(
            &writer_pool,
            &SessionToken::from_bytes([2; 32]),
            &FixedClock(1100),
            review(Locale::Nl, "paused"),
        )
        .await
    });
    waiting_on(&pool, "SELECT pg_advisory_xact_lock%").await;
    let revoke_pool = pool.clone();
    let revoke = tokio::spawn(async move {
        sqlx::query("DELETE FROM staff_capabilities WHERE staff_id='reader' AND capability='catalogue.review'").execute(&revoke_pool).await
    });
    waiting_on(&pool, "DELETE FROM staff_capabilities%").await;
    assert!(!revoke.is_finished());
    blocker.rollback().await.expect("resume authorized review");
    writer
        .await
        .expect("review task")
        .expect("review commits before revocation");
    assert_eq!(
        revoke
            .await
            .expect("revocation task")
            .expect("revoke after commit")
            .rows_affected(),
        1
    );
    let reader = SessionToken::from_bytes([2; 32]);
    for key in ["paused", "new-effect"] {
        assert!(matches!(
            record_review(&pool, &reader, &FixedClock(1100), review(Locale::Nl, key)).await,
            Err(TranslationError::Staff(StaffError::Denied))
        ));
    }
    assert!(matches!(
        translation_for_review(
            &pool,
            &reader,
            &FixedClock(1100),
            "DEMO-FILLED",
            Locale::Nl,
            1
        )
        .await,
        Err(TranslationError::Staff(StaffError::Denied))
    ));
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cancelled_copy_write_releases_its_intent_and_retry_has_one_complete_effect(pool: PgPool) {
    seed(&pool).await;
    record_translation(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        request(&copy(), Locale::Nl, "source"),
    )
    .await
    .expect("source");
    let mut blocker = pool.begin().await.expect("variant blocker");
    blocker
        .execute("UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku='DEMO-FILLED'")
        .await
        .expect("pause draft");
    let writer_pool = pool.clone();
    let writer = tokio::spawn(async move {
        record_translation(
            &writer_pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            request(&copy(), Locale::De, "retry-cancelled"),
        )
        .await
    });
    waiting_on(&pool, "UPDATE catalogue_variants%RETURNING sku").await;
    writer.abort();
    assert!(writer.await.expect_err("cancelled").is_cancelled());
    blocker.rollback().await.expect("release cancelled writer");
    assert_eq!(counts(&pool).await, (1, 0));
    let result = record_translation(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        request(&copy(), Locale::De, "retry-cancelled"),
    )
    .await
    .expect("recover cancelled effect");
    assert_eq!((result.revision, result.replayed), (1, false));
    assert_eq!(counts(&pool).await, (2, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn schema_rejects_history_rewrites_gaps_and_unbound_review_claims(pool: PgPool) {
    seed(&pool).await;
    record_translation(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        request(&copy(), Locale::Nl, "copy"),
    )
    .await
    .expect("copy");
    record_review(
        &pool,
        &SessionToken::from_bytes([2; 32]),
        &FixedClock(1100),
        review(Locale::Nl, "review"),
    )
    .await
    .expect("review");
    for statement in [
        "UPDATE catalogue_translation_revisions SET payload=payload",
        "DELETE FROM catalogue_translation_revisions",
        "UPDATE catalogue_translation_reviews SET decision=decision",
        "DELETE FROM catalogue_translation_reviews",
        "INSERT INTO catalogue_translation_revisions(sku,locale,revision,specification_revision,source_revision,effect_key,payload,actor,reason) SELECT sku,locale,3,1,NULL,'gap',payload,'author','fixture' FROM catalogue_translation_revisions",
        "INSERT INTO catalogue_translation_revisions(sku,locale,revision,specification_revision,source_revision,effect_key,payload,actor,reason) SELECT sku,'ku',1,1,1,'wrong-locale',payload,'author','fixture' FROM catalogue_translation_revisions",
        "INSERT INTO catalogue_translation_revisions(sku,locale,revision,specification_revision,source_revision,effect_key,payload,actor,reason) SELECT sku,'fr',1,1,NULL,'no-source',payload,'author','fixture' FROM catalogue_translation_revisions",
        "INSERT INTO catalogue_translation_revisions(sku,locale,revision,specification_revision,source_revision,effect_key,payload,actor,reason) SELECT sku,'fr',1,1,99,'wrong-source',payload,'author','fixture' FROM catalogue_translation_revisions",
        "INSERT INTO catalogue_translation_revisions(sku,locale,revision,specification_revision,source_revision,effect_key,payload,actor,reason) SELECT sku,'nl',2,1,1,'source-on-dutch',payload,'author','fixture' FROM catalogue_translation_revisions",
        "INSERT INTO catalogue_translation_reviews(sku,locale,translation_revision,revision,effect_key,decision,evidence_ref,qualification_ref,actor,reason) VALUES('DEMO-FILLED','nl',1,3,'gap-review','approved','fixture','fixture','reader','fixture')",
        "INSERT INTO catalogue_translation_reviews(sku,locale,translation_revision,revision,effect_key,decision,evidence_ref,qualification_ref,actor,reason) VALUES('DEMO-FILLED','nl',99,1,'missing-copy','approved','fixture','fixture','reader','fixture')",
        "INSERT INTO catalogue_translation_reviews(sku,locale,translation_revision,revision,effect_key,decision,evidence_ref,qualification_ref,actor,reason) VALUES('DEMO-FILLED','nl',1,2,'no-qualification','approved','fixture',' ','reader','fixture')",
    ] {
        let error = pool.execute(statement).await.expect_err("schema boundary");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514"),
            "{statement}"
        );
    }
    let mut too_large = copy();
    too_large.title = "\u{1}".repeat(160);
    too_large.description = "\u{1}".repeat(4000);
    too_large.materials = "\u{1}".repeat(2000);
    too_large.care = "\u{1}".repeat(2000);
    too_large.safety = "\u{1}".repeat(2000);
    too_large.assembly = "\u{1}".repeat(2000);
    assert!(too_large.validate().is_ok());
    let mut request = request(&too_large, Locale::Nl, "over-byte-budget");
    request.expected_revision = 1;
    assert!(matches!(
        record_translation(
            &pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            request
        )
        .await,
        Err(TranslationError::InvalidRequest)
    ));
    assert_eq!(counts(&pool).await, (1, 1));
}

async fn replace_payload(pool: &PgPool, locale: &str, payload: &str) {
    pool.execute(
        "ALTER TABLE catalogue_translation_revisions DISABLE TRIGGER guard_translation_revision",
    )
    .await
    .expect("isolated corruption setup");
    sqlx::query("UPDATE catalogue_translation_revisions SET payload=$1::jsonb WHERE sku='DEMO-FILLED' AND locale=$2").bind(payload).bind(locale).execute(pool).await.expect("corrupt fixture only");
    pool.execute(
        "ALTER TABLE catalogue_translation_revisions ENABLE TRIGGER guard_translation_revision",
    )
    .await
    .expect("restore guard");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn malformed_target_or_dutch_source_and_broken_references_never_support_review(pool: PgPool) {
    seed(&pool).await;
    let author = SessionToken::from_bytes([1; 32]);
    let reader = SessionToken::from_bytes([2; 32]);
    let clock = FixedClock(1100);
    let content = copy();
    record_translation(&pool, &author, &clock, request(&content, Locale::Nl, "nl"))
        .await
        .expect("source");
    record_translation(&pool, &author, &clock, request(&content, Locale::De, "de"))
        .await
        .expect("target");
    let original = serde_json::to_string(&content).expect("content");
    for (locale,payload) in [("de","{}".into()),("nl","{}".into()),("nl",serde_json::json!({"title":"fixture","description":"fixture","materials":"fixture","care":" ","safety":"fixture","assembly":"fixture"}).to_string()),("nl",serde_json::json!({"title":"fixture","description":"fixture","materials":"fixture","care":"fixture","safety":"fixture","assembly":"fixture","reviewed":true}).to_string())] {
        replace_payload(&pool,locale,&payload).await;
        assert!(matches!(translation(&pool,&author,&clock,"DEMO-FILLED",Locale::De,1).await,Err(TranslationError::InvalidStoredData)));
        assert!(matches!(record_review(&pool,&reader,&clock,review(Locale::De,"damaged-copy")).await,Err(TranslationError::InvalidStoredData)));
        if locale=="nl" {
            assert!(matches!(record_translation(&pool,&author,&clock,request(&content,Locale::Fr,"damaged-source")).await,Err(TranslationError::InvalidStoredData)));
        }
        replace_payload(&pool,locale,&original).await;
    }
    pool.execute("ALTER TABLE catalogue_translation_revisions DROP CONSTRAINT translation_source_fk; ALTER TABLE catalogue_translation_revisions DISABLE TRIGGER guard_translation_revision; UPDATE catalogue_translation_revisions SET source_revision=99 WHERE locale='de'; ALTER TABLE catalogue_translation_revisions ENABLE TRIGGER guard_translation_revision").await.expect("isolated broken reference");
    assert!(matches!(
        translation(&pool, &author, &clock, "DEMO-FILLED", Locale::De, 1).await,
        Err(TranslationError::InvalidStoredData)
    ));
    pool.execute("ALTER TABLE catalogue_translation_revisions DISABLE TRIGGER guard_translation_revision; UPDATE catalogue_translation_revisions SET source_revision=1 WHERE locale='de'; ALTER TABLE catalogue_translation_revisions ENABLE TRIGGER guard_translation_revision; ALTER TABLE catalogue_translation_revisions ADD CONSTRAINT translation_source_fk FOREIGN KEY(sku,source_locale,source_revision) REFERENCES catalogue_translation_revisions(sku,locale,revision)").await.expect("restore fixture reference");
    pool.execute("ALTER TABLE catalogue_translation_revisions DROP CONSTRAINT translation_source_shape; ALTER TABLE catalogue_translation_revisions DISABLE TRIGGER guard_translation_revision; UPDATE catalogue_translation_revisions SET source_revision=1 WHERE locale='nl'; ALTER TABLE catalogue_translation_revisions ENABLE TRIGGER guard_translation_revision").await.expect("isolated source-shape corruption");
    assert!(matches!(
        translation(&pool, &author, &clock, "DEMO-FILLED", Locale::De, 1).await,
        Err(TranslationError::InvalidStoredData)
    ));
    pool.execute("ALTER TABLE catalogue_translation_revisions DISABLE TRIGGER guard_translation_revision; UPDATE catalogue_translation_revisions SET source_revision=NULL WHERE locale='nl'; ALTER TABLE catalogue_translation_revisions ENABLE TRIGGER guard_translation_revision; ALTER TABLE catalogue_translation_revisions ADD CONSTRAINT translation_source_shape CHECK((locale='nl' AND source_revision IS NULL) OR (locale<>'nl' AND source_revision IS NOT NULL AND source_revision>0))").await.expect("restore source shape");
    record_review(&pool, &reader, &clock, review(Locale::Nl, "valid-review"))
        .await
        .expect("review after repair");
    pool.execute("ALTER TABLE catalogue_translation_reviews DISABLE TRIGGER guard_translation_review; UPDATE catalogue_translation_reviews SET actor='author'; ALTER TABLE catalogue_translation_reviews ENABLE TRIGGER guard_translation_review").await.expect("isolated forged independence");
    assert!(matches!(
        translation(&pool, &author, &clock, "DEMO-FILLED", Locale::Nl, 1).await,
        Err(TranslationError::InvalidStoredData)
    ));
    assert_eq!(counts(&pool).await, (2, 1));
}

#[sqlx::test(migrations = false)]
async fn schema_upgrade_keeps_legacy_copy_and_does_not_grant_review_or_seed_approvals(
    pool: PgPool,
) {
    MIGRATOR
        .run_to(11, &pool)
        .await
        .expect("prior media/image schema");
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("prior fixture");
    pool.execute("INSERT INTO catalogue_translations(sku,locale,title,description,care) VALUES('DEMO-FILLED','nl','prior fixture title','prior fixture description','prior fixture care'); INSERT INTO staff_accounts VALUES('prior-author',true); INSERT INTO staff_capabilities VALUES('prior-author','catalogue.manage')").await.expect("prior copy and least privilege");
    let token = SessionToken::from_bytes([7; 32]);
    sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,'prior-author',1000,4600,1100)").bind(token.digest().as_slice()).execute(&pool).await.expect("prior test-only authority");
    record_specification(
        &pool,
        &token,
        &FixedClock(1100),
        SpecificationRequest {
            sku: "DEMO-FILLED",
            bom_revision: 1,
            expected_revision: 0,
            effect_key: "prior-source",
            draft: &draft(true),
            reason: "prior isolated source",
            confirmed: true,
        },
    )
    .await
    .expect("prior source");
    let before = commerce_infrastructure::catalogue(&pool, Locale::Nl)
        .await
        .expect("prior reader");
    assert_eq!(before.len(), 1);
    MIGRATOR
        .run(&pool)
        .await
        .expect("expand copy/review schema");
    assert_eq!(counts(&pool).await, (0, 0));
    let grants: Vec<(String, String)> =
        sqlx::query_as("SELECT staff_id,capability FROM staff_capabilities")
            .fetch_all(&pool)
            .await
            .expect("grant oracle");
    assert_eq!(grants, [("prior-author".into(), "catalogue.manage".into())]);
    assert!(matches!(
        record_review(
            &pool,
            &token,
            &FixedClock(1100),
            review(Locale::Nl, "no-review-authority")
        )
        .await,
        Err(TranslationError::Staff(StaffError::Denied))
    ));
    record_translation(
        &pool,
        &token,
        &FixedClock(1100),
        request(&copy(), Locale::Nl, "after-upgrade"),
    )
    .await
    .expect("new guarded draft");
    assert_eq!(
        commerce_infrastructure::catalogue(&pool, Locale::Nl)
            .await
            .expect("compatible old reader"),
        before
    );
    assert_eq!(counts(&pool).await, (1, 0));
}
