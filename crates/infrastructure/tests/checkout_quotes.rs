use commerce_application::Locale;
use commerce_domain::{guest_baskets::BasketUpdate, quotes::BasketLine};
use commerce_infrastructure::{MIGRATOR, checkout_quotes::*, guest_baskets::*};
use sqlx::{Executor, PgPool};
use std::sync::atomic::Ordering;
#[path = "support/checkout_quote_fixture.rs"]
mod fixture;
use fixture::*;
#[sqlx::test(migrator = "MIGRATOR")]
async fn owner_snapshot_persists_exact_original_money_sources_and_idempotent_receipt(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = store(&pool, 1100);
    let (s, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let receipt = issue_quote(&store, &key, &csrf, Locale::Nl, &request("one"))
        .await
        .expect("snapshot");
    assert!(!receipt.replayed);
    let value = receipt.snapshot;
    assert_eq!(
        (
            value.binding.basket_revision,
            value.binding.policy_revision,
            value.binding.policy_review_revision,
            value.binding.created_at,
            value.binding.expires_at
        ),
        (1, 1, 1, 1100, 1400)
    );
    assert_eq!(
        (
            value.lines[0].publication_revision,
            value.lines[0].bom_revision,
            value.total.gross_minor,
            value.total.net_minor,
            value.total.vat_minor
        ),
        (1, 1, 38245, 31607, 6638)
    );
    let replay = issue_quote(&store, &key, &csrf, Locale::Nl, &request("one"))
        .await
        .expect("retry");
    assert!(replay.replayed);
    assert!(replay.snapshot == value);
    assert!(
        owned_quote(&store, &key, &value.binding.quote_id)
            .await
            .expect("own read")
            == value
    );
    assert_eq!(counts(&pool).await, (1, 0, 0));
    for sql in [
        "UPDATE guest_quote_snapshots SET expires_at=expires_at+1",
        "UPDATE guest_quote_snapshots SET snapshot=snapshot",
        "DELETE FROM guest_quote_snapshots",
    ] {
        let error = pool.execute(sql).await.expect_err("immutable quote");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514")
        );
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn two_guests_cannot_read_or_mutate_each_others_quotes_and_revocation_blocks_history(
    pool: PgPool,
) {
    seed(&pool).await;
    let (store, _) = store(&pool, 1100);
    let (a, ac) = guest(&store).await;
    let (b, bc) = guest(&store).await;
    let ak = GuestCredential::from_session(&a).expect("A");
    let bk = GuestCredential::from_session(&b).expect("B");
    assert!(matches!(
        issue_quote(&store, &ak, &bc, Locale::Nl, &request("cross")).await,
        Err(QuoteStoreError::Guest(GuestError::Csrf))
    ));
    let quote = issue_quote(&store, &ak, &ac, Locale::Nl, &request("one"))
        .await
        .expect("A owns")
        .snapshot;
    assert!(matches!(
        owned_quote(&store, &bk, &quote.binding.quote_id).await,
        Err(QuoteStoreError::NotFound)
    ));
    revoke_session(&store, &a, &ac).await.expect("revoked");
    assert!(matches!(
        owned_quote(&store, &ak, &quote.binding.quote_id).await,
        Err(QuoteStoreError::Guest(GuestError::Denied))
    ));
    assert!(matches!(
        issue_quote(&store, &ak, &ac, Locale::Nl, &request("one")).await,
        Err(QuoteStoreError::Guest(GuestError::Denied))
    ));
    assert_eq!(counts(&pool).await, (1, 0, 0));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn historical_effect_does_not_revert_current_basket_or_refresh_an_expired_quote(
    pool: PgPool,
) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (s, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let first = issue_quote(&store, &key, &csrf, Locale::Nl, &request("one"))
        .await
        .expect("first")
        .snapshot;
    write_basket(
        &store,
        &key,
        &csrf,
        &BasketUpdate {
            expected_revision: 1,
            effect_key: "empty".into(),
            lines: vec![],
        },
    )
    .await
    .expect("newer empty intent");
    let old = issue_quote(&store, &key, &csrf, Locale::Nl, &request("one"))
        .await
        .expect("exact historical receipt");
    assert!(old.replayed);
    assert!(old.snapshot == first);
    assert!(
        read_basket(&store, &key)
            .await
            .expect("current intent")
            .lines
            .is_empty()
    );
    assert!(matches!(
        issue_quote(&store, &key, &csrf, Locale::Nl, &request("different")).await,
        Err(QuoteStoreError::Conflict)
    ));
    let mut changed = request("one");
    changed.expected_basket_revision = 2;
    assert!(matches!(
        issue_quote(&store, &key, &csrf, Locale::Nl, &changed).await,
        Err(QuoteStoreError::Conflict)
    ));
    changed = request("one");
    changed.destination.postcode = "1234 AC".into();
    assert!(matches!(
        issue_quote(&store, &key, &csrf, Locale::Nl, &changed).await,
        Err(QuoteStoreError::Conflict)
    ));
    assert!(matches!(
        issue_quote(&store, &key, &csrf, Locale::Kmr, &request("one")).await,
        Err(QuoteStoreError::Conflict)
    ));
    time.0.store(1400, Ordering::SeqCst);
    assert!(matches!(
        owned_quote(&store, &key, &first.binding.quote_id).await,
        Err(QuoteStoreError::Expired)
    ));
    assert!(matches!(
        issue_quote(&store, &key, &csrf, Locale::Nl, &request("one")).await,
        Err(QuoteStoreError::Expired)
    ));
    assert_eq!(counts(&pool).await, (1, 0, 0));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn successful_issuance_ceiling_is_shared_without_replay_charges_or_new_stock(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (s, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    for n in 0..10 {
        issue_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &request(&format!("quote-{n}")),
        )
        .await
        .expect("bounded quotes");
    }
    assert!(matches!(
        issue_quote(&store, &key, &csrf, Locale::Nl, &request("exhausted")).await,
        Err(QuoteStoreError::RateLimited)
    ));
    assert!(
        issue_quote(&store, &key, &csrf, Locale::Nl, &request("quote-0"))
            .await
            .expect("no new quota")
            .replayed
    );
    time.0.store(1160, Ordering::SeqCst);
    issue_quote(&store, &key, &csrf, Locale::Nl, &request("next-window"))
        .await
        .expect("next minute");
    time.0.store(1100, Ordering::SeqCst);
    assert!(matches!(
        issue_quote(&store, &key, &csrf, Locale::Nl, &request("rewind")).await,
        Err(QuoteStoreError::ClockUnavailable)
    ));
    assert_eq!(counts(&pool).await, (11, 0, 0));
}
async fn waiting(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(900),async{loop{
  let reached:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(pattern).fetch_one(pool).await.expect("lock observer");if reached{return;}tokio::time::sleep(std::time::Duration::from_millis(5)).await;
 }}).await.expect("quote reached real lock wait");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn post_insert_policy_expiry_and_cancellation_never_commit_quote_or_resource_counter(
    pool: PgPool,
) {
    seed(&pool).await;
    let mut p = policy();
    p.revision = 2;
    p.expires_at = 1200;
    draft(&pool, &p).await;
    decide(&pool, 2, 0, true, "short-policy").await;
    pool.execute("CREATE FUNCTION pause_quote_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(851233); RETURN NEW; END; $$; CREATE TRIGGER pause_quote_insert AFTER INSERT ON guest_quote_snapshots FOR EACH ROW EXECUTE FUNCTION pause_quote_insert()").await.expect("post-write timing instrument");
    let (store, time) = store(&pool, 1100);
    let (session, csrf) = guest(&store).await;
    for mode in ["cancel", "expired", "reverse", "positive"] {
        time.0.store(1100, Ordering::SeqCst);
        let mut blocker = pool.begin().await.expect("insert gate");
        sqlx::query("SELECT pg_advisory_xact_lock(851233)")
            .execute(&mut *blocker)
            .await
            .expect("hold gate");
        let (ws, key, token) = (
            store.clone(),
            GuestCredential::from_session(&session).expect("key"),
            csrf.clone(),
        );
        let work = tokio::spawn(async move {
            issue_quote(&ws, &key, &token, Locale::Nl, &request(mode)).await
        });
        waiting(&pool, "INSERT INTO guest_quote_snapshots%").await;
        if mode == "cancel" {
            work.abort();
            assert!(work.await.err().expect("cancelled").is_cancelled());
            blocker.rollback().await.expect("unlock");
        } else {
            if mode == "expired" {
                time.0.store(1200, Ordering::SeqCst);
            }
            if mode == "reverse" {
                time.0.store(1099, Ordering::SeqCst);
            }
            blocker.rollback().await.expect("resume");
            let result = work.await.expect("task");
            match mode {
                "expired" => assert!(matches!(
                    result,
                    Err(QuoteStoreError::Evaluation(
                        commerce_infrastructure::quotes::QuoteFailure::Policy(
                            commerce_domain::quotes::QuoteError::MarketUnavailable
                        )
                    ))
                )),
                "reverse" => assert!(matches!(
                    result,
                    Err(QuoteStoreError::Guest(GuestError::ClockUnavailable))
                )),
                _ => assert!(result.is_ok()),
            }
        }
        assert_eq!(counts(&pool).await, (i64::from(mode == "positive"), 0, 0));
        let quota: i64 =
            sqlx::query_scalar("SELECT coalesce(sum(issued),0)::bigint FROM guest_quote_quota")
                .fetch_one(&pool)
                .await
                .expect("atomic quota");
        assert_eq!(quota, i64::from(mode == "positive"));
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn snapshot_deadline_and_photo_rights_are_rechecked_after_actual_insert_wait(pool: PgPool) {
    seed(&pool).await;
    pool.execute("CREATE FUNCTION pause_quote_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(851234); RETURN NEW; END; $$; CREATE TRIGGER pause_quote_insert AFTER INSERT ON guest_quote_snapshots FOR EACH ROW EXECUTE FUNCTION pause_quote_insert()").await.expect("timing instrument");
    let (store, time) = store(&pool, 1100);
    let (session, csrf) = guest(&store).await;
    for (start, end) in [(1100, 1400), (1900, 2000)] {
        time.0.store(start, Ordering::SeqCst);
        let (store, csrf) = (store.clone(), csrf.clone());
        let key = GuestCredential::from_session(&session).expect("key");
        let mut blocker = pool.begin().await.expect("gate");
        sqlx::query("SELECT pg_advisory_xact_lock(851234)")
            .execute(&mut *blocker)
            .await
            .expect("lock");
        let work = tokio::spawn(async move {
            issue_quote(&store, &key, &csrf, Locale::Nl, &request("wait")).await
        });
        waiting(&pool, "INSERT INTO guest_quote_snapshots%").await;
        time.0.store(end, Ordering::SeqCst);
        blocker.rollback().await.expect("resume");
        let result = work.await.expect("task");
        if start == 1100 {
            assert!(matches!(result, Err(QuoteStoreError::Expired)));
        } else {
            assert!(matches!(
                result,
                Err(QuoteStoreError::Evaluation(
                    commerce_infrastructure::quotes::QuoteFailure::Policy(
                        commerce_domain::quotes::QuoteError::ProductUnavailable
                    )
                ))
            ));
        }
        assert_eq!(counts(&pool).await, (0, 0, 0));
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn revocation_waits_for_quote_commit_and_then_blocks_historical_access(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = store(&pool, 1100);
    let (session, csrf) = guest(&store).await;
    let original = GuestCredential::from_session(&session).expect("key");
    let mut blocker = pool.begin().await.expect("stock blocker");
    sqlx::query("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("physical stock held");
    let (ws, key, token) = (
        store.clone(),
        GuestCredential::from_session(&session).expect("key"),
        csrf.clone(),
    );
    let work =
        tokio::spawn(
            async move { issue_quote(&ws, &key, &token, Locale::Nl, &request("one")).await },
        );
    waiting(
        &pool,
        "SELECT component_code,on_hand,quarantine,reserved FROM stock_balances%",
    )
    .await;
    let (rs, token) = (store.clone(), csrf.clone());
    let revoke = tokio::spawn(async move { revoke_session(&rs, &session, &token).await });
    waiting(&pool, "SELECT left(basket_id%FOR UPDATE").await;
    blocker.rollback().await.expect("resume quote");
    let quote = work
        .await
        .expect("quote task")
        .expect("authorized before revoke")
        .snapshot;
    revoke
        .await
        .expect("revoke task")
        .expect("revoked after commit");
    assert!(matches!(
        owned_quote(&store, &original, &quote.binding.quote_id).await,
        Err(QuoteStoreError::Guest(GuestError::Denied))
    ));
    assert_eq!(counts(&pool).await, (1, 0, 0));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_exact_effects_have_one_snapshot_and_unready_sources_never_fall_back(
    pool: PgPool,
) {
    seed(&pool).await;
    let (store, _) = store(&pool, 1100);
    let (session, csrf) = guest(&store).await;
    let a = GuestCredential::from_session(&session).expect("A");
    let b = GuestCredential::from_session(&session).expect("B");
    let input = request("same");
    let (x, y) = tokio::join!(
        issue_quote(&store, &a, &csrf, Locale::Nl, &input),
        issue_quote(&store, &b, &csrf, Locale::Nl, &input)
    );
    let (x, y) = (x.expect("one"), y.expect("retry"));
    assert_ne!(x.replayed, y.replayed);
    assert!(x.snapshot == y.snapshot);
    assert_eq!(counts(&pool).await, (1, 0, 0));
    decide(&pool, 1, 1, false, "withdraw").await;
    assert!(matches!(
        issue_quote(&store, &a, &csrf, Locale::Nl, &request("new")).await,
        Err(QuoteStoreError::Market(
            commerce_infrastructure::market_policies::MarketError::NotFound
        ))
    ));
    assert!(
        issue_quote(&store, &a, &csrf, Locale::Nl, &input)
            .await
            .expect("history grants no new quote authority")
            .replayed
    );
    let mut changed = request("new-destination");
    changed.destination.country = "DE".into();
    assert!(matches!(
        issue_quote(&store, &a, &csrf, Locale::Nl, &changed).await,
        Err(QuoteStoreError::Market(
            commerce_infrastructure::market_policies::MarketError::NotFound
        ))
    ));
    assert_eq!(counts(&pool).await, (1, 0, 0));
    write_basket(
        &store,
        &a,
        &csrf,
        &BasketUpdate {
            expected_revision: 1,
            effect_key: "changed".into(),
            lines: vec![BasketLine {
                sku: "SET-FILLED".into(),
                quantity: 1,
            }],
        },
    )
    .await
    .expect("new intent");
    assert_eq!(
        read_basket(&store, &a)
            .await
            .expect("retained newer")
            .revision,
        2
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn raw_imports_require_exact_basket_review_publication_and_bom_bindings(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = store(&pool, 1100);
    let (session, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("key");
    let original = issue_quote(&store, &key, &csrf, Locale::Nl, &request("source"))
        .await
        .expect("quote")
        .snapshot;
    for case in 0..13 {
        let mut s = original.clone();
        s.binding.quote_id = format!("{:032x}", 1000 + case);
        let mut value = serde_json::to_value(&s).expect("snapshot");
        match case {
            0 => value["binding"]["quote_id"] = "wrong".into(),
            1 => value["schema_version"] = 2.into(),
            2 => value["currency"] = "USD".into(),
            3 => value["lines"] = serde_json::json!({}),
            4 => value["lines"] = serde_json::json!([]),
            5 => {
                s.binding.policy_review_revision = 99;
                value["binding"]["policy_review_revision"] = 99.into();
            }
            6 => {
                s.binding.postcode = "9999 XX".into();
                value["binding"]["postcode"] = "9999 XX".into();
            }
            7 => value["lines"][0]["quantity"] = 1.into(),
            8 => value["lines"][0]["publication_revision"] = 99.into(),
            9 => value["lines"][0]["bom_revision"] = 99.into(),
            10 => {
                s.binding.basket_revision = 99;
                value["binding"]["basket_revision"] = 99.into();
            }
            12 => {
                s.binding.created_at = 1200;
                s.binding.expires_at = 1500;
                value["binding"]["created_at"] = 1200.into();
                value["binding"]["expires_at"] = 1500.into();
            }
            _ => {}
        }
        let b = &s.binding;
        let result=sqlx::query("INSERT INTO guest_quote_snapshots(id,basket_id,basket_revision,effect_key,locale,country,postcode,policy_id,policy_revision,policy_review_revision,created_at,expires_at,snapshot) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)")
            .bind(&b.quote_id).bind(&b.basket_id).bind(b.basket_revision).bind(format!("import-{case}"))
            .bind(&b.locale).bind(&b.country).bind(&b.postcode).bind(&b.policy_id).bind(b.policy_revision)
            .bind(b.policy_review_revision).bind(b.created_at).bind(b.expires_at).bind(value).execute(&pool).await;
        if case >= 11 {
            result.expect("positive exact immutable import control");
            let read = owned_quote(&store, &key, &b.quote_id).await;
            if case == 11 {
                assert!(read.expect("revalidated import") == s);
            } else {
                assert!(matches!(read, Err(QuoteStoreError::ClockUnavailable)));
            }
        } else {
            let error = result.expect_err("malformed import denied");
            assert!(
                error
                    .as_database_error()
                    .and_then(|e| e.code())
                    .is_some_and(|code| code.starts_with("23")),
                "case {case}: {error}"
            );
        }
    }
    assert_eq!(counts(&pool).await, (3, 0, 0));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_intent_empty_cart_and_failed_entropy_never_issue_snapshots(pool: PgPool) {
    use commerce_application::{
        guest_access::GuestLimits,
        request_context::{ContextError, Identifiers},
    };
    use std::sync::Arc;
    struct Failed;
    impl Identifiers for Failed {
        fn next(&self) -> Result<[u8; 16], ContextError> {
            Err(ContextError::EntropyUnavailable)
        }
    }
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (session, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("key");
    for n in 0..4 {
        let mut input = request("bad");
        match n {
            0 => input.expected_basket_revision = 0,
            1 => input.effect_key = " ".into(),
            2 => input.destination.country = "XX".into(),
            _ => input.destination.postcode = "".into(),
        };
        assert!(matches!(
            issue_quote(&store, &key, &csrf, Locale::Nl, &input).await,
            Err(QuoteStoreError::InvalidRequest)
        ));
    }
    for id in ["bad", "G0000000000000000000000000000000"] {
        assert!(matches!(
            owned_quote(&store, &key, id).await,
            Err(QuoteStoreError::InvalidRequest)
        ));
    }
    assert!(matches!(
        owned_quote(&store, &key, &"9".repeat(32)).await,
        Err(QuoteStoreError::NotFound)
    ));
    let no_entropy = GuestSessionStore::new(
        pool.clone(),
        time.clone(),
        Arc::new(Failed),
        GuestLimits::default(),
    )
    .expect("store");
    assert!(matches!(
        issue_quote(&no_entropy, &key, &csrf, Locale::Nl, &request("no-entropy")).await,
        Err(QuoteStoreError::Guest(GuestError::EntropyUnavailable))
    ));
    let quota: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_quote_quota")
        .fetch_one(&pool)
        .await
        .expect("quota rollback");
    assert_eq!(quota, 0);
    write_basket(
        &store,
        &key,
        &csrf,
        &BasketUpdate {
            expected_revision: 1,
            effect_key: "empty".into(),
            lines: vec![],
        },
    )
    .await
    .expect("empty intent");
    let mut input = request("empty");
    input.expected_basket_revision = 2;
    assert!(matches!(
        issue_quote(&store, &key, &csrf, Locale::Nl, &input).await,
        Err(QuoteStoreError::InvalidRequest)
    ));
    time.0.store(1200, Ordering::SeqCst);
    write_basket(
        &store,
        &key,
        &csrf,
        &BasketUpdate {
            expected_revision: 2,
            effect_key: "later".into(),
            lines: vec![],
        },
    )
    .await
    .expect("later mutation window");
    time.0.store(1100, Ordering::SeqCst);
    assert!(matches!(
        owned_quote(&store, &key, &"9".repeat(32)).await,
        Err(QuoteStoreError::Guest(GuestError::ClockUnavailable))
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_quota_storage_and_dependency_outage_never_grant_a_quote(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = store(&pool, 1100);
    let (session, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("key");
    issue_quote(&store, &key, &csrf, Locale::Nl, &request("positive"))
        .await
        .expect("positive");
    pool.execute("ALTER TABLE guest_quote_quota DROP CONSTRAINT guest_quote_quota_rate_window_check, DROP CONSTRAINT guest_quote_quota_issued_check").await.expect("damaged restore fixture");
    for (window, issued) in [(-1, 1), (18, -1), (18, 11)] {
        sqlx::query("UPDATE guest_quote_quota SET rate_window=$1,issued=$2")
            .bind(window as i64)
            .bind(issued)
            .execute(&pool)
            .await
            .expect("damage");
        assert!(matches!(
            issue_quote(&store, &key, &csrf, Locale::Nl, &request("damaged")).await,
            Err(QuoteStoreError::InvalidStoredData)
        ));
    }
    assert_eq!(counts(&pool).await, (1, 0, 0));
    pool.execute("DROP TABLE guest_quote_snapshots CASCADE")
        .await
        .expect("missing dependency");
    assert!(matches!(
        owned_quote(&store, &key, &"9".repeat(32)).await,
        Err(QuoteStoreError::Database(_))
    ));
    assert!(matches!(
        issue_quote(&store, &key, &csrf, Locale::Nl, &request("outage")).await,
        Err(QuoteStoreError::Database(_))
    ));
}
