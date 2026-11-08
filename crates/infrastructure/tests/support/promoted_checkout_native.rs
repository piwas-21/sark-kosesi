//! Native isolation controls for indicative promotion source custody.
use super::*;
use sqlx::{Postgres, Transaction};

fn check_violation(error: sqlx::Error, expected: &str) {
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514"),
        "native guard must reject the source"
    );
    assert!(
        error
            .as_database_error()
            .is_some_and(|e| e.message() == expected),
        "unexpected native guard"
    );
}

async fn issued(pool: &PgPool) -> String {
    seed(pool).await;
    let (store, _) = fixture::store(pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("fixture owner");
    issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("native-original"),
        "COURTYARD",
    )
    .await
    .expect("genuine approved promoted quote")
    .snapshot
    .original
    .binding
    .quote_id
}

async fn clone_quote(
    tx: &mut Transaction<'_, Postgres>,
    old: &str,
    new: &str,
    changed_reference: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO guest_quote_snapshots
         (id,basket_id,basket_revision,effect_key,locale,country,postcode,policy_id,
          policy_revision,policy_review_revision,created_at,expires_at,snapshot,
          promotion_id,promotion_revision,promotion_review_revision)
         SELECT $2,basket_id,basket_revision,'native-copy',locale,country,postcode,policy_id,
          policy_revision,policy_review_revision,created_at,expires_at,
          CASE WHEN $3 THEN jsonb_set(jsonb_set(snapshot,'{original,binding,quote_id}',to_jsonb($2::text)),
                '{promotion,revision}','2'::jsonb)
           ELSE jsonb_set(snapshot,'{original,binding,quote_id}',to_jsonb($2::text)) END,
          promotion_id,promotion_revision,promotion_review_revision
         FROM guest_quote_snapshots WHERE id=$1"#,
    )
    .bind(old)
    .bind(new)
    .bind(changed_reference)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exact_promotion_reference_and_order_binding_fail_closed(pool: PgPool) {
    let quote = issued(&pool).await;
    let mut wrong = pool.begin().await.expect("bad reference transaction");
    check_violation(
        clone_quote(&mut wrong, &quote, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", true)
            .await
            .expect_err("mismatched nested review source is rejected"),
        "Promoted quote requires a bounded closed envelope",
    );
    wrong.rollback().await.expect("discard rejected insert");

    let mut good = pool.begin().await.expect("positive reference transaction");
    clone_quote(&mut good, &quote, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", false)
        .await
        .expect("same exact source is accepted");
    good.rollback().await.expect("discard positive control");

    let mut order_tx = pool.begin().await.expect("legacy order transaction");
    let error = sqlx::query(
        r#"INSERT INTO checkout_orders
           (id,basket_id,basket_revision,quote_id,effect_key,stock_group_id,
            created_at,hold_expires_at,intent,snapshot)
           SELECT repeat('c',32),basket_id,basket_revision,id,'native-order',repeat('d',32),
                  created_at,created_at+100,'{}'::jsonb,'{}'::jsonb
           FROM guest_quote_snapshots WHERE id=$1"#,
    )
    .bind(&quote)
    .execute(&mut *order_tx)
    .await
    .expect_err("promoted order requires an exact intent and snapshot");
    check_violation(
        error,
        "Order requires exact owned quote, intent and source snapshots",
    );
    order_tx.rollback().await.expect("discard rejected order");
    // Model a damaged import in this isolated database only: even without the
    // reference columns, the version-two envelope must never enter checkout.
    pool.execute("ALTER TABLE guest_quote_snapshots DISABLE TRIGGER guard_guest_quote")
        .await
        .expect("isolate simulated imported corruption");
    sqlx::query("UPDATE guest_quote_snapshots SET promotion_id=NULL,promotion_revision=NULL,promotion_review_revision=NULL,promotion_transaction_id=NULL WHERE id=$1")
        .bind(&quote).execute(&pool).await.expect("simulate missing imported promotion metadata");
    pool.execute("ALTER TABLE guest_quote_snapshots ENABLE TRIGGER guard_guest_quote")
        .await
        .expect("restore immutable quote guard");
    let mut corrupt = pool.begin().await.expect("corrupt quote order transaction");
    let error = sqlx::query(
        r#"INSERT INTO checkout_orders
           (id,basket_id,basket_revision,quote_id,effect_key,stock_group_id,
            created_at,hold_expires_at,intent,snapshot)
           SELECT repeat('c',32),basket_id,basket_revision,id,'native-order',repeat('d',32),
                  created_at,created_at+100,'{}'::jsonb,'{}'::jsonb
           FROM guest_quote_snapshots WHERE id=$1"#,
    )
    .bind(&quote)
    .execute(&mut *corrupt)
    .await
    .expect_err("version-two envelope is rejected even with missing promotion metadata");
    check_violation(error, "Legacy order requires a legacy quote");
    corrupt
        .rollback()
        .await
        .expect("discard corrupt quote order");
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM guest_quote_snapshots),(SELECT count(*) FROM checkout_orders)",
    )
    .fetch_one(&pool)
    .await
    .expect("unchanged quote and order counts");
    assert_eq!(counts, (1, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn quote_seals_same_transaction_withdrawal_but_separate_withdrawal_is_allowed(pool: PgPool) {
    let quote = issued(&pool).await;
    let mut tx = pool
        .begin()
        .await
        .expect("quote and withdrawal transaction");
    clone_quote(&mut tx, &quote, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", false)
        .await
        .expect("current approval admits fresh quote");
    let sealed: bool = sqlx::query_scalar("SELECT promotion_transaction_id=pg_current_xact_id() FROM guest_quote_snapshots WHERE id=repeat('b',32)")
        .fetch_one(&mut *tx).await.expect("native transaction stamp");
    assert!(sealed);
    let error = sqlx::query(
        "INSERT INTO promotion_policy_reviews(id,draft_revision,revision,decision,evidence_ref,qualification_ref,effect_key,actor,reason) VALUES('COURTYARD',1,2,'rejected','fixture','fixture','native-withdraw','reviewer','fixture')",
    )
    .execute(&mut *tx)
    .await
    .expect_err("same transaction cannot withdraw after quote insertion");
    check_violation(
        error,
        "Promoted quote source is sealed for this transaction",
    );
    tx.rollback()
        .await
        .expect("discard quote and failed withdrawal");
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM guest_quote_snapshots),(SELECT count(*) FROM promotion_policy_reviews WHERE id='COURTYARD')")
        .fetch_one(&pool).await.expect("atomic rollback");
    assert_eq!(counts, (1, 1));
    promotion::decide(&pool, "separate-withdraw", 1, 1, false)
        .await
        .expect("later independent withdrawal remains available");
    let decision: String = sqlx::query_scalar("SELECT decision FROM promotion_policy_reviews WHERE id='COURTYARD' AND draft_revision=1 AND revision=2")
        .fetch_one(&pool).await.expect("withdrawal committed");
    assert_eq!(decision, "rejected");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn committed_withdrawal_across_anchor_wait_rejects_quote_and_quota(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("fixture owner");
    let mut writer = pool.begin().await.expect("promotion writer");
    sqlx::query("SELECT id FROM promotion_policy_anchors WHERE id='COURTYARD' FOR UPDATE")
        .execute(&mut *writer)
        .await
        .expect("hold policy anchor");
    let request = fixture::request("wait-withdraw");
    let waiter = issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD");
    let withdraw = async {
        tokio::time::timeout(std::time::Duration::from_millis(800), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%promotion_policy_anchors%FOR SHARE%')")
                    .fetch_one(&pool).await.expect("inspect lock wait");
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("quote reached anchor wait");
        sqlx::query("INSERT INTO promotion_policy_reviews(id,draft_revision,revision,decision,evidence_ref,qualification_ref,effect_key,actor,reason) VALUES('COURTYARD',1,2,'rejected','fixture','fixture','wait-withdraw','reviewer','fixture')")
            .execute(&mut *writer).await.expect("commit new review under held anchor");
        writer.commit().await.expect("publish withdrawal");
    };
    let (result, ()) = tokio::join!(waiter, withdraw);
    assert!(result.is_err(), "stale policy cannot issue a quote");
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM guest_quote_snapshots),(SELECT coalesce(sum(issued),0)::bigint FROM guest_quote_quota)")
        .fetch_one(&pool).await.expect("quote and quota rollback");
    assert_eq!(counts, (0, 0));
    promotion::decide(&pool, "wait-approve-again", 1, 2, true)
        .await
        .expect("independent reapproval");
    issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD")
        .await
        .expect("same intent succeeds after fresh approval");
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM guest_quote_snapshots),(SELECT coalesce(sum(issued),0)::bigint FROM guest_quote_quota)")
        .fetch_one(&pool).await.expect("positive quote and quota");
    assert_eq!(counts, (1, 1));
}

#[sqlx::test(migrations = false)]
async fn populated_schema47_upgrade_preserves_owned_v1_and_accepts_first_v2(pool: PgPool) {
    MIGRATOR.run_to(47, &pool).await.expect("schema 47");
    seed(&pool).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("fixture owner");
    let legacy = issue_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("upgrade-legacy"),
    )
    .await
    .expect("persisted version-one quote before upgrade")
    .snapshot;
    let id = &legacy.binding.quote_id;
    let before: (String, String, i32, String, i64, i64) = sqlx::query_as(
        "SELECT snapshot::text,basket_id,basket_revision,effect_key,created_at,expires_at FROM guest_quote_snapshots WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("captured exact legacy row bytes and binding");
    // Before integration this applies provisional 0049 after 0047. After the
    // pending 0048 lands, the same call applies both migrations in order.
    MIGRATOR
        .run_to(49, &pool)
        .await
        .expect("upgrade to schema 49");
    type UpgradedLegacyRow = (
        String,
        String,
        i32,
        String,
        i64,
        i64,
        Option<String>,
        Option<i32>,
        Option<i32>,
        bool,
    );
    let after: UpgradedLegacyRow = sqlx::query_as(
        "SELECT snapshot::text,basket_id,basket_revision,effect_key,created_at,expires_at,promotion_id,promotion_revision,promotion_review_revision,promotion_transaction_id IS NULL FROM guest_quote_snapshots WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("legacy row after upgrade");
    assert_eq!(
        (&after.0, &after.1, after.2, &after.3, after.4, after.5),
        (
            &before.0, &before.1, before.2, &before.3, before.4, before.5
        )
    );
    assert_eq!(
        (after.6, after.7, after.8, after.9),
        (None, None, None, true)
    );
    assert!(
        owned_quote(&store, &key, id)
            .await
            .expect("owned legacy receipt")
            == legacy
    );
    assert!(matches!(
        owned_promoted_quote(&store, &key, id).await,
        Err(QuoteStoreError::NotFound)
    ));
    let promoted = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("upgrade-promoted"),
        "COURTYARD",
    )
    .await
    .expect("first promoted quote after upgrade")
    .snapshot;
    assert_eq!(
        (promoted.schema_version, promoted.original.schema_version),
        (2, 1)
    );
    assert!(
        owned_promoted_quote(&store, &key, &promoted.original.binding.quote_id)
            .await
            .expect("owned promoted receipt")
            == promoted
    );
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM guest_quote_snapshots),(SELECT coalesce(sum(issued),0)::bigint FROM guest_quote_quota)")
        .fetch_one(&pool).await.expect("shared history and quota");
    assert_eq!(counts, (2, 2));
    MIGRATOR
        .run(&pool)
        .await
        .expect("upgrade through current redemption schema");
    let legacy_order = commerce_infrastructure::checkout_orders::create_order(
        &store,
        &key,
        &csrf,
        "upgrade-legacy-order",
        &order::intent(id),
        &order::seller(),
    )
    .await
    .expect("legacy quote still creates an order after upgrade");
    assert!(legacy_order.snapshot.promoted_quote.is_none());
    let promoted_order = commerce_infrastructure::checkout_orders::create_order(
        &store,
        &key,
        &csrf,
        "upgrade-promoted-order",
        &order::intent(&promoted.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("promoted quote creates its debit after upgrade");
    assert!(promoted_order.snapshot.promoted_quote.as_ref() == Some(&promoted));
    let debit_count: i64 = sqlx::query_scalar("SELECT count(*) FROM promotion_redemptions")
        .fetch_one(&pool)
        .await
        .expect("one promoted debit");
    assert_eq!(debit_count, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn promotion_expiry_after_insert_wait_rolls_back_quote_and_quota(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("fixture owner");
    pool.execute(
        "CREATE FUNCTION promoted_quote_test_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(1936876904,1); RETURN NEW; END; $$",
    )
    .await
    .expect("isolated blocking trigger function");
    pool.execute("CREATE TRIGGER z_promoted_quote_test_wait AFTER INSERT ON guest_quote_snapshots FOR EACH ROW WHEN (NEW.promotion_id IS NOT NULL) EXECUTE FUNCTION promoted_quote_test_wait()")
        .await
        .expect("wait after promoted quote insertion");
    let mut blocker = pool.begin().await.expect("expiry blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876904,1)")
        .execute(&mut *blocker)
        .await
        .expect("hold isolated advisory lock");
    let request = fixture::request("expiry-after-insert");
    let waiter = issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD");
    let expire = async {
        tokio::time::timeout(std::time::Duration::from_millis(800), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%INSERT INTO guest_quote_snapshots%')")
                    .fetch_one(&pool).await.expect("inspect actual post-insert wait");
                if waiting {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("quote reached post-insert trigger wait");
        time.0.store(2000, Ordering::SeqCst);
        blocker.rollback().await.expect("release expiry blocker");
    };
    let (result, ()) = tokio::join!(waiter, expire);
    assert!(result.is_err(), "expired source cannot issue a quote");
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM guest_quote_snapshots),(SELECT coalesce(sum(issued),0)::bigint FROM guest_quote_quota)")
        .fetch_one(&pool).await.expect("expired quote and quota roll back");
    assert_eq!(counts, (0, 0));
    pool.execute("DROP TRIGGER z_promoted_quote_test_wait ON guest_quote_snapshots")
        .await
        .expect("remove isolated blocking trigger");
    time.0.store(1100, Ordering::SeqCst);
    issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD")
        .await
        .expect("fresh operation succeeds during eligible window");
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM guest_quote_snapshots),(SELECT coalesce(sum(issued),0)::bigint FROM guest_quote_quota)")
        .fetch_one(&pool).await.expect("fresh quote and quota");
    assert_eq!(counts, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_or_changed_v2_readback_rolls_back_quota_and_allows_retry(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("fixture owner");
    let template = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("readback-template"),
        "COURTYARD",
    )
    .await
    .expect("healthy promoted fixture")
    .snapshot;
    let counts = || async {
        sqlx::query_as::<_, (i64, i64)>("SELECT (SELECT count(*) FROM guest_quote_snapshots),(SELECT coalesce(sum(issued),0)::bigint FROM guest_quote_quota)")
            .fetch_one(&pool).await.expect("quote and quota counts")
    };
    assert_eq!(counts().await, (1, 1));

    pool.execute("CREATE FUNCTION promoted_quote_suppress_test() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END; $$")
        .await.expect("isolated suppression fault");
    pool.execute("CREATE TRIGGER z_promoted_quote_suppress_test BEFORE INSERT ON guest_quote_snapshots FOR EACH ROW WHEN (NEW.promotion_id IS NOT NULL) EXECUTE FUNCTION promoted_quote_suppress_test()")
        .await.expect("suppress promoted inserts");
    let suppressed = fixture::request("readback-suppressed");
    assert!(
        issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &suppressed, "COURTYARD")
            .await
            .is_err(),
        "suppressed insert cannot be acknowledged"
    );
    assert_eq!(counts().await, (1, 1));
    pool.execute("DROP TRIGGER z_promoted_quote_suppress_test ON guest_quote_snapshots")
        .await
        .expect("remove suppression fault");
    issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &suppressed, "COURTYARD")
        .await
        .expect("same key succeeds after suppressed insert");
    assert_eq!(counts().await, (2, 2));

    pool.execute("CREATE FUNCTION promoted_quote_shorten_test() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.expires_at=NEW.expires_at-1; NEW.snapshot=jsonb_set(NEW.snapshot,'{original,binding,expires_at}',to_jsonb(NEW.expires_at)); RETURN NEW; END; $$")
        .await.expect("isolated altered readback fault");
    pool.execute("CREATE TRIGGER z_promoted_quote_shorten_test BEFORE INSERT ON guest_quote_snapshots FOR EACH ROW WHEN (NEW.promotion_id IS NOT NULL) EXECUTE FUNCTION promoted_quote_shorten_test()")
        .await.expect("shorten both stored expiry fields");
    // A raw temporary insert proves this is a different, internally valid v2
    // snapshot, rather than a malformed payload rejected by the decoder.
    let mut control = pool.begin().await.expect("changed-snapshot control");
    clone_quote(
        &mut control,
        &template.original.binding.quote_id,
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        false,
    )
    .await
    .expect("native guard accepts exact current source");
    let (expiry, value): (i64, serde_json::Value) = sqlx::query_as(
        "SELECT expires_at,snapshot FROM guest_quote_snapshots WHERE id=repeat('e',32)",
    )
    .fetch_one(&mut *control)
    .await
    .expect("shortened control snapshot");
    let changed: commerce_domain::checkout_quotes::PromotedQuoteSnapshot =
        serde_json::from_value(value).expect("closed promoted snapshot");
    assert_eq!(expiry, template.original.binding.expires_at - 1);
    assert_eq!(changed.original.binding.expires_at, expiry);
    assert!(changed.validate(&promotion::policy(), 1).is_ok());
    control.rollback().await.expect("discard changed control");

    let changed_request = fixture::request("readback-changed");
    assert!(matches!(
        issue_promoted_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &changed_request,
            "COURTYARD"
        )
        .await,
        Err(QuoteStoreError::InvalidStoredData)
    ));
    assert_eq!(counts().await, (2, 2));
    pool.execute("DROP TRIGGER z_promoted_quote_shorten_test ON guest_quote_snapshots")
        .await
        .expect("remove altered readback fault");
    issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &changed_request,
        "COURTYARD",
    )
    .await
    .expect("same key succeeds after altered readback");
    assert_eq!(counts().await, (3, 3));
}
