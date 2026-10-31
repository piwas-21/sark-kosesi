use commerce_application::request_context::{Clock, ContextError};
use commerce_domain::{factual_review::FactDecision, promotions::DiscountKind};
use commerce_infrastructure::{MIGRATOR, promotion_policies::*, staff_access::StaffError};
use sqlx::{Executor, PgPool};
#[path = "support/promotion_policy_fixture.rs"]
mod fixture;
use fixture::*;
#[sqlx::test(migrator = "MIGRATOR")]
async fn current_independent_review_and_exact_history_never_restore_withdrawn_promotion(
    pool: PgPool,
) {
    seed(&pool).await;
    assert!(matches!(approved(&pool).await, Err(PolicyError::NotFound)));
    let first = draft(&pool, &policy(), "first").await.expect("draft");
    assert_eq!(first.revision, 1);
    assert!(!first.replayed);
    let p = inspect(&pool, 1).await.expect("pending");
    assert_eq!(p.policy, policy());
    assert!(p.review.is_none());
    assert_eq!(p.author, "author");
    assert_eq!(p.reason, "synthetic promotion");
    assert!(matches!(approved(&pool).await, Err(PolicyError::NotFound)));
    decide(&pool, "approve", 1, 0, true)
        .await
        .expect("independent");
    let p = approved(&pool).await.expect("approved");
    assert_eq!(p.policy, policy());
    assert_eq!((p.review_revision, p.evaluated_at), (1, 1100));
    let r = inspect(&pool, 1)
        .await
        .expect("inspect")
        .review
        .expect("review");
    assert!(r.approved);
    assert_eq!(r.reviewer, "reviewer");
    assert_eq!(r.evidence_ref, "synthetic review");
    assert_eq!(r.qualification_ref, "synthetic qualification");
    decide(&pool, "reject", 1, 1, false)
        .await
        .expect("withdraw");
    assert!(matches!(approved(&pool).await, Err(PolicyError::NotFound)));
    assert!(
        decide(&pool, "approve", 1, 0, true)
            .await
            .expect("old receipt")
            .replayed
    );
    assert!(matches!(approved(&pool).await, Err(PolicyError::NotFound)));
    let mut next = policy();
    next.promotion.revision = 2;
    next.promotion.discount = DiscountKind::Percentage { basis_points: 1000 };
    draft(&pool, &next, "next").await.expect("next draft");
    assert!(matches!(approved(&pool).await, Err(PolicyError::NotFound)));
    assert!(matches!(
        decide(&pool, "stale-approval", 1, 2, true).await,
        Err(PolicyError::Conflict)
    ));
    decide(&pool, "new-approval", 2, 0, true)
        .await
        .expect("current approval");
    assert_eq!(approved(&pool).await.expect("new").policy, next);
    assert!(
        draft(&pool, &policy(), "first")
            .await
            .expect("old draft receipt")
            .replayed
    );
    assert_eq!(approved(&pool).await.expect("still new").policy, next);
    assert!(
        !inspect(&pool, 1)
            .await
            .expect("history")
            .review
            .expect("withdrawal")
            .approved
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn separate_capabilities_confirmation_mfa_and_independent_actor_are_mandatory(pool: PgPool) {
    seed(&pool).await;
    for n in [2, 3, 4, 99] {
        assert!(matches!(
            record_draft(
                &pool,
                &token(n),
                &clock(),
                DraftRequest {
                    policy: &policy(),
                    audit: audit("denied", 0)
                }
            )
            .await,
            Err(PolicyError::Staff(StaffError::Denied))
        ));
    }
    let mut a = audit("unconfirmed", 0);
    a.confirmed = false;
    assert!(matches!(
        record_draft(
            &pool,
            &token(1),
            &clock(),
            DraftRequest {
                policy: &policy(),
                audit: a
            }
        )
        .await,
        Err(PolicyError::InvalidRequest)
    ));
    draft(&pool, &policy(), "source").await.expect("source");
    for n in [1, 4, 99] {
        assert!(matches!(
            record_review(&pool, &token(n), &clock(), review("denied", 1, 0, true)).await,
            Err(PolicyError::Staff(StaffError::Denied))
        ));
    }
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.review')")
        .await
        .expect("dual role fixture");
    assert!(matches!(
        record_review(&pool, &token(1), &clock(), review("self", 1, 0, true)).await,
        Err(PolicyError::SelfReview)
    ));
    pool.execute("UPDATE staff_sessions SET mfa_verified_at=NULL WHERE staff_id='reviewer'")
        .await
        .expect("missing MFA");
    assert!(matches!(
        decide(&pool, "no-mfa", 1, 0, true).await,
        Err(PolicyError::Staff(StaffError::Denied))
    ));
    assert!(matches!(
        inspect_policy(&pool, &token(4), &clock(), "COURTYARD", 1).await,
        Err(PolicyError::Staff(StaffError::Denied))
    ));
    assert!(matches!(approved(&pool).await, Err(PolicyError::NotFound)));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn effects_bind_normalized_full_terms_limits_actor_and_review_intent(pool: PgPool) {
    ready(&pool).await;
    let mut equivalent = policy();
    equivalent.promotion.countries.reverse();
    assert!(
        draft(&pool, &equivalent, "initial")
            .await
            .expect("canonical retry")
            .replayed
    );
    for n in 0..4 {
        let mut changed = policy();
        match n {
            0 => changed.limits.total_discount_minor = 9999,
            1 => changed.limits.total_redemptions = 19,
            2 => changed.limits.per_customer_redemptions = Some(1),
            _ => changed.promotion.maximum_discount_minor = 499,
        };
        assert!(matches!(
            draft(&pool, &changed, "initial").await,
            Err(PolicyError::Conflict)
        ));
    }
    let mut r = review("approve", 1, 0, true);
    r.evidence_ref = "changed";
    assert!(matches!(
        record_review(&pool, &token(2), &clock(), r).await,
        Err(PolicyError::Conflict)
    ));
    assert!(matches!(
        record_review(&pool, &token(3), &clock(), review("approve", 1, 0, true)).await,
        Err(PolicyError::Conflict)
    ));
    let mut r = review("approve", 1, 0, true);
    r.decision = FactDecision::Rejected;
    assert!(matches!(
        record_review(&pool, &token(2), &clock(), r).await,
        Err(PolicyError::Conflict)
    ));
    assert_eq!(approved(&pool).await.expect("unchanged").policy, policy());
}
struct Sequence(std::sync::Mutex<std::collections::VecDeque<Result<u64, ContextError>>>);
impl Sequence {
    fn new(values: Vec<Result<u64, ContextError>>) -> Self {
        Self(std::sync::Mutex::new(values.into()))
    }
}
impl Clock for Sequence {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        self.0
            .lock()
            .expect("clock")
            .pop_front()
            .expect("scripted sample")
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn one_observation_rejects_backward_and_failed_clocks_for_writes_reads_and_replay(
    pool: PgPool,
) {
    seed(&pool).await;
    let reverse = || Sequence::new(vec![Ok(1101), Ok(1100)]);
    assert!(matches!(
        record_draft(
            &pool,
            &token(1),
            &reverse(),
            DraftRequest {
                policy: &policy(),
                audit: audit("reverse", 0)
            }
        )
        .await,
        Err(PolicyError::Staff(StaffError::ClockUnavailable))
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM promotion_policy_anchors")
        .fetch_one(&pool)
        .await
        .expect("rollback");
    assert_eq!(count, 0);
    draft(&pool, &policy(), "initial").await.expect("source");
    assert!(matches!(
        record_review(&pool, &token(2), &reverse(), review("reverse", 1, 0, true)).await,
        Err(PolicyError::Staff(StaffError::ClockUnavailable))
    ));
    decide(&pool, "approved", 1, 0, true).await.expect("review");
    assert!(matches!(
        inspect_policy(&pool, &token(2), &reverse(), "COURTYARD", 1).await,
        Err(PolicyError::Staff(StaffError::ClockUnavailable))
    ));
    assert!(matches!(
        approved_policy(&pool, &reverse(), "COURTYARD", "NL").await,
        Err(PolicyError::ClockUnavailable)
    ));
    assert!(matches!(
        record_draft(
            &pool,
            &token(1),
            &reverse(),
            DraftRequest {
                policy: &policy(),
                audit: audit("initial", 0)
            }
        )
        .await,
        Err(PolicyError::Staff(StaffError::ClockUnavailable))
    ));
    assert!(matches!(
        record_review(&pool, &token(2), &reverse(), review("approved", 1, 0, true)).await,
        Err(PolicyError::Staff(StaffError::ClockUnavailable))
    ));
    for sample in [Err(ContextError::ClockUnavailable), Ok(u64::MAX)] {
        assert!(matches!(
            approved_policy(&pool, &Sequence::new(vec![sample]), "COURTYARD", "NL").await,
            Err(PolicyError::ClockUnavailable)
        ));
        assert!(matches!(
            inspect_policy(
                &pool,
                &token(2),
                &Sequence::new(vec![sample]),
                "COURTYARD",
                1
            )
            .await,
            Err(PolicyError::Staff(StaffError::ClockUnavailable))
        ));
    }
    assert_eq!(approved(&pool).await.expect("unchanged").review_revision, 1);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_drafts_and_reviews_have_one_complete_winner_and_exact_replay(pool: PgPool) {
    seed(&pool).await;
    let p = policy();
    let (a, b) = tokio::join!(draft(&pool, &p, "same"), draft(&pool, &p, "same"));
    let (a, b) = (a.expect("one"), b.expect("retry"));
    assert_ne!(a.replayed, b.replayed);
    let mut x = policy();
    x.promotion.revision = 2;
    x.limits.total_discount_minor = 5000;
    let mut y = x.clone();
    y.limits.total_discount_minor = 6000;
    let (a, b) = tokio::join!(draft(&pool, &x, "x"), draft(&pool, &y, "y"));
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(PolicyError::Conflict)) || matches!(b, Err(PolicyError::Conflict)));
    let winner = inspect(&pool, 2).await.expect("complete winner").policy;
    assert!(winner == x || winner == y);
    let (a, b) = tokio::join!(
        decide(&pool, "yes", 2, 0, true),
        decide(&pool, "no", 2, 0, false)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(PolicyError::Conflict)) || matches!(b, Err(PolicyError::Conflict)));
    let count:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM promotion_policy_drafts),(SELECT count(*) FROM promotion_policy_reviews)").fetch_one(&pool).await.expect("history");
    assert_eq!(count, (2, 1));
}
async fn waiting(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(900),async{loop{
 let yes:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(pattern).fetch_one(pool).await.expect("observer");if yes{return;}tokio::time::sleep(std::time::Duration::from_millis(5)).await;
 }}).await.expect("real promotion lock wait");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn final_authority_after_insert_wait_and_cancellation_leave_no_history(pool: PgPool) {
    use std::sync::{Arc, atomic::Ordering};
    seed(&pool).await;
    pool.execute("UPDATE staff_sessions SET expires_at=1200 WHERE staff_id='author'; CREATE FUNCTION pause_promotion_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(851235); RETURN NEW; END; $$; CREATE TRIGGER pause_promotion_write AFTER INSERT ON promotion_policy_drafts FOR EACH ROW EXECUTE FUNCTION pause_promotion_write()").await.expect("timing instrument");
    let time = Arc::new(clock());
    for mode in ["cancel", "expired", "reverse", "positive"] {
        time.0.store(1101, Ordering::SeqCst);
        let mut blocker = pool.begin().await.expect("gate");
        sqlx::query("SELECT pg_advisory_xact_lock(851235)")
            .execute(&mut *blocker)
            .await
            .expect("hold");
        let (wp, wt) = (pool.clone(), time.clone());
        let work = tokio::spawn(async move {
            record_draft(
                &wp,
                &token(1),
                wt.as_ref(),
                DraftRequest {
                    policy: &policy(),
                    audit: audit(mode, 0),
                },
            )
            .await
        });
        waiting(&pool, "INSERT INTO promotion_policy_drafts%").await;
        if mode == "cancel" {
            work.abort();
            assert!(work.await.expect_err("cancelled").is_cancelled());
            blocker.rollback().await.expect("unlock");
        } else {
            time.0.store(
                match mode {
                    "expired" => 1200,
                    "reverse" => 1100,
                    _ => 1102,
                },
                Ordering::SeqCst,
            );
            blocker.rollback().await.expect("resume");
            let result = work.await.expect("task");
            match mode {
                "expired" => assert!(matches!(
                    result,
                    Err(PolicyError::Staff(StaffError::Denied))
                )),
                "reverse" => assert!(matches!(
                    result,
                    Err(PolicyError::Staff(StaffError::ClockUnavailable))
                )),
                _ => assert!(result.is_ok()),
            }
        }
        let count:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM promotion_policy_anchors),(SELECT count(*) FROM promotion_policy_drafts)").fetch_one(&pool).await.expect("atomic source");
        let n = i64::from(mode == "positive");
        assert_eq!(count, (n, n));
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn promotion_selection_holds_anchor_through_reads_and_withdrawal_waits(pool: PgPool) {
    ready(&pool).await;
    let mut blocker = pool.begin().await.expect("table gate");
    sqlx::query("LOCK TABLE promotion_policy_drafts IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .expect("hold draft reads");
    let wp = pool.clone();
    let read = tokio::spawn(async move { approved(&wp).await });
    waiting(&pool, "SELECT max(revision) FROM promotion_policy_drafts%").await;
    let wp = pool.clone();
    let withdrawal = tokio::spawn(async move { decide(&wp, "withdraw", 1, 1, false).await });
    waiting(
        &pool,
        "UPDATE promotion_policy_anchors SET id=id%RETURNING id",
    )
    .await;
    blocker.rollback().await.expect("resume");
    assert_eq!(
        read.await
            .expect("read task")
            .expect("selected before withdrawal")
            .review_revision,
        1
    );
    withdrawal.await.expect("writer").expect("withdrawn");
    assert!(matches!(approved(&pool).await, Err(PolicyError::NotFound)));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn promotion_deadline_is_checked_after_source_wait_and_unknown_destinations_fail_closed(
    pool: PgPool,
) {
    use std::sync::{Arc, atomic::Ordering};
    ready(&pool).await;
    for country in ["BE", "GB", ""] {
        let result = approved_policy(&pool, &clock(), "COURTYARD", country).await;
        if country == "BE" {
            assert!(matches!(result, Err(PolicyError::NotFound)));
        } else {
            assert!(matches!(result, Err(PolicyError::InvalidRequest)));
        }
    }
    let time = Arc::new(clock());
    let mut blocker = pool.begin().await.expect("anchor gate");
    sqlx::query("UPDATE promotion_policy_anchors SET id=id")
        .execute(&mut *blocker)
        .await
        .expect("hold");
    let (wp, wt) = (pool.clone(), time.clone());
    let work =
        tokio::spawn(async move { approved_policy(&wp, wt.as_ref(), "COURTYARD", "NL").await });
    waiting(&pool, "SELECT id FROM promotion_policy_anchors%FOR SHARE").await;
    time.0.store(2000, Ordering::SeqCst);
    blocker.rollback().await.expect("resume");
    assert!(matches!(
        work.await.expect("task"),
        Err(PolicyError::NotFound)
    ));
    assert!(approved(&pool).await.is_ok());
}
#[path = "support/promotion_policy_schema.rs"]
mod schema;
#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_commands_missing_sources_and_storage_outages_never_grant_approval(pool: PgPool) {
    ready(&pool).await;
    for n in 0..4 {
        let mut a = audit("bad", 0);
        match n {
            0 => a.expected_revision = -1,
            1 => a.effect_key = "",
            2 => a.reason = "",
            _ => a.confirmed = false,
        };
        assert!(matches!(
            record_draft(
                &pool,
                &token(1),
                &clock(),
                DraftRequest {
                    policy: &policy(),
                    audit: a
                }
            )
            .await,
            Err(PolicyError::InvalidRequest)
        ));
    }
    let mut invalid = policy();
    invalid.limits.total_redemptions = 0;
    assert!(matches!(
        draft(&pool, &invalid, "bad-policy").await,
        Err(PolicyError::InvalidRequest)
    ));
    let mut p = policy();
    p.promotion.revision = 3;
    assert!(matches!(
        draft(&pool, &p, "stale").await,
        Err(PolicyError::Conflict)
    ));
    for n in 0..5 {
        let mut r = review("invalid", 1, 0, true);
        match n {
            0 => r.id = "DEMO",
            1 => r.draft_revision = 0,
            2 => r.evidence_ref = "",
            3 => r.qualification_ref = "",
            _ => r.audit.expected_revision = -1,
        };
        assert!(matches!(
            record_review(&pool, &token(2), &clock(), r).await,
            Err(PolicyError::InvalidRequest)
        ));
    }
    for (id, revision) in [("DEMO", 1), ("COURTYARD", 0)] {
        assert!(matches!(
            inspect_policy(&pool, &token(2), &clock(), id, revision).await,
            Err(PolicyError::InvalidRequest)
        ));
    }
    let mut r = review("unknown", 1, 0, true);
    r.id = "UNKNOWN";
    assert!(matches!(
        record_review(&pool, &token(2), &clock(), r).await,
        Err(PolicyError::NotFound)
    ));
    assert!(matches!(
        record_review(
            &pool,
            &token(2),
            &clock(),
            review("unknown-draft", 99, 0, false)
        )
        .await,
        Err(PolicyError::NotFound)
    ));
    pool.execute("INSERT INTO promotion_policy_anchors VALUES('EMPTY')")
        .await
        .expect("empty configured anchor");
    assert!(matches!(
        approved_policy(&pool, &clock(), "EMPTY", "NL").await,
        Err(PolicyError::NotFound)
    ));
    pool.execute("DROP TABLE promotion_policy_reviews CASCADE")
        .await
        .expect("storage outage fixture");
    assert!(matches!(
        inspect(&pool, 1).await,
        Err(PolicyError::Database(_))
    ));
    assert!(matches!(
        approved(&pool).await,
        Err(PolicyError::Database(_))
    ));
    assert!(matches!(
        decide(&pool, "outage", 1, 1, false).await,
        Err(PolicyError::Database(_))
    ));
    pool.execute("DROP TABLE promotion_policy_drafts CASCADE")
        .await
        .expect("draft outage");
    assert!(matches!(
        draft(&pool, &policy(), "initial").await,
        Err(PolicyError::Database(_))
    ));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn staff_grant_revocation_waits_for_atomic_draft_and_then_denies_historical_replay(
    pool: PgPool,
) {
    ready(&pool).await;
    let mut blocker = pool.begin().await.expect("anchor gate");
    sqlx::query("UPDATE promotion_policy_anchors SET id=id")
        .execute(&mut *blocker)
        .await
        .expect("hold");
    let wp = pool.clone();
    let work = tokio::spawn(async move {
        let mut p = policy();
        p.promotion.revision = 2;
        draft(&wp, &p, "second").await
    });
    waiting(&pool, "INSERT INTO promotion_policy_anchors%").await;
    let wp = pool.clone();
    let revoke = tokio::spawn(async move {
        sqlx::query("DELETE FROM staff_capabilities WHERE staff_id='author' AND capability='promotion.manage'").execute(&wp).await
    });
    waiting(&pool, "DELETE FROM staff_capabilities%").await;
    blocker.rollback().await.expect("resume");
    work.await.expect("writer task").expect("authorized draft");
    revoke.await.expect("revoke task").expect("revoked");
    let mut p = policy();
    p.promotion.revision = 2;
    assert!(matches!(
        draft(&pool, &p, "second").await,
        Err(PolicyError::Staff(StaffError::Denied))
    ));
    assert_eq!(inspect(&pool, 2).await.expect("retained history").policy, p);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn all_historical_receipt_headers_bind_original_identity_revision_and_audit(pool: PgPool) {
    ready(&pool).await;
    let mut other = policy();
    other.promotion.id = "OTHER".into();
    assert!(matches!(
        draft(&pool, &other, "initial").await,
        Err(PolicyError::Conflict)
    ));
    let mut next = policy();
    next.promotion.revision = 2;
    assert!(matches!(
        draft(&pool, &next, "initial").await,
        Err(PolicyError::Conflict)
    ));
    pool.execute("INSERT INTO staff_capabilities VALUES('other','promotion.manage')")
        .await
        .expect("second author fixture");
    assert!(matches!(
        record_draft(
            &pool,
            &token(3),
            &clock(),
            DraftRequest {
                policy: &policy(),
                audit: audit("initial", 0)
            }
        )
        .await,
        Err(PolicyError::Conflict)
    ));
    let mut changed = audit("initial", 0);
    changed.reason = "changed";
    assert!(matches!(
        record_draft(
            &pool,
            &token(1),
            &clock(),
            DraftRequest {
                policy: &policy(),
                audit: changed
            }
        )
        .await,
        Err(PolicyError::Conflict)
    ));
    let mismatch = audit("bad-revision", 1);
    assert!(matches!(
        record_draft(
            &pool,
            &token(1),
            &clock(),
            DraftRequest {
                policy: &policy(),
                audit: mismatch
            }
        )
        .await,
        Err(PolicyError::InvalidRequest)
    ));
    let too_long = "x".repeat(501);
    let mut invalid = audit("long-reason", 0);
    invalid.reason = &too_long;
    assert!(matches!(
        record_draft(
            &pool,
            &token(1),
            &clock(),
            DraftRequest {
                policy: &policy(),
                audit: invalid
            }
        )
        .await,
        Err(PolicyError::InvalidRequest)
    ));
    draft(&pool, &other, "other-source")
        .await
        .expect("other definition");
    let mut r = review("approve", 1, 0, true);
    r.id = "OTHER";
    assert!(matches!(
        record_review(&pool, &token(2), &clock(), r).await,
        Err(PolicyError::Conflict)
    ));
    draft(&pool, &next, "next-source").await.expect("new draft");
    assert!(matches!(
        record_review(&pool, &token(2), &clock(), review("approve", 2, 0, true)).await,
        Err(PolicyError::Conflict)
    ));
    assert!(matches!(
        record_review(&pool, &token(2), &clock(), review("approve", 1, 1, true)).await,
        Err(PolicyError::Conflict)
    ));
    let mut r = review("approve", 1, 0, true);
    r.qualification_ref = "changed";
    assert!(matches!(
        record_review(&pool, &token(2), &clock(), r).await,
        Err(PolicyError::Conflict)
    ));
    let mut r = review("approve", 1, 0, true);
    r.audit.reason = "changed";
    assert!(matches!(
        record_review(&pool, &token(2), &clock(), r).await,
        Err(PolicyError::Conflict)
    ));
    assert!(matches!(
        approved_policy(&pool, &clock(), "DEMO", "NL").await,
        Err(PolicyError::InvalidRequest)
    ));
    assert!(
        decide(&pool, "approve", 1, 0, true)
            .await
            .expect("unchanged historical receipt")
            .replayed
    );
    assert!(matches!(approved(&pool).await, Err(PolicyError::NotFound)));
    assert!(
        inspect(&pool, 2)
            .await
            .expect("new pending")
            .review
            .is_none()
    );
    let counts:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM promotion_policy_drafts),(SELECT count(*) FROM promotion_policy_reviews)").fetch_one(&pool).await.expect("no new effects");
    assert_eq!(counts, (3, 1));
}
