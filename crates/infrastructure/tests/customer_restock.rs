//! Private restock opt-in controls. All facts and identities are synthetic.
#[path = "support/admitted_login.rs"]
mod admitted_login;
#[allow(dead_code)]
#[path = "support/admitted_proofs.rs"]
mod admitted_proofs;
#[allow(dead_code, unused_imports)]
mod catalogue {
    include!("../../../tests/fixtures/approved_catalogue.rs");
}

use admitted_proofs::{
    customer_passwords, email_verification, verified_customer_accounts::establish,
};
use catalogue::*;
use commerce_infrastructure::{
    customer_favourites,
    customer_restock::{self, RestockError, RestockItem},
    customer_sessions::{self, CustomerSessionToken},
};

async fn account(pool: &PgPool, email: &str) -> (i64, CustomerSessionToken) {
    let clock = FixedClock(1100);
    let proof = email_verification::issue(pool, email, &clock)
        .await
        .expect("email proof");
    let id = establish(pool, email, &proof, &clock)
        .await
        .expect("account")
        .value();
    let proof = customer_passwords::issue(pool, email, &clock)
        .await
        .expect("password proof");
    customer_passwords::replace(pool, email, &proof, "correct horse battery staple", &clock)
        .await
        .expect("password");
    let token = admitted_login::login(pool, email, "correct horse battery staple", &clock)
        .await
        .expect("login")
        .token;
    (id, token)
}
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

#[sqlx::test(migrator = "MIGRATOR")]
async fn explicit_opt_in_is_private_versioned_and_cancellable_when_hidden(pool: PgPool) {
    let _fixture = media_seed(&pool).await;
    let (id, first) = account(&pool, "first@example.test").await;
    let (_, other) = account(&pool, "other@example.test").await;
    let clock = FixedClock(1100);
    assert_eq!(
        customer_restock::subscribe(&pool, &first, &clock, Locale::Nl, "SET-FILLED", 0).await,
        Err(RestockError::Unavailable)
    );
    assert_eq!(
        customer_restock::subscribe(&pool, &first, &clock, Locale::Nl, "DEMO-COVERS", 0).await,
        Err(RestockError::InvalidRequest)
    );
    activate(&pool).await;
    let before: (i64,i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM stock_balances),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM email_verification_delivery_starts)").fetch_one(&pool).await.expect("unrelated effects");
    // Out-of-stock is allowed; no alert is sent or promised.
    assert!(
        !commerce_infrastructure::published_catalogue::published_product(
            &pool,
            &clock,
            Locale::Nl,
            "SET-FILLED"
        )
        .await
        .expect("projection")
        .expect("product")
        .in_stock
    );
    // A later database trigger that silently reverses the opt-in must roll the
    // whole request back rather than fabricate a successful subscription.
    pool.execute("CREATE FUNCTION undo_restock_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN DELETE FROM customer_restock_subscriptions WHERE account_id=NEW.account_id AND sku=NEW.sku; RETURN NEW; END $$; CREATE TRIGGER zzz_undo_restock_insert AFTER INSERT ON customer_restock_subscriptions FOR EACH ROW EXECUTE FUNCTION undo_restock_insert()").await.expect("install isolated post-insert fault");
    assert_eq!(
        customer_restock::subscribe(&pool, &first, &clock, Locale::Nl, "SET-FILLED", 0).await,
        Err(RestockError::Unavailable)
    );
    let after_fault = customer_restock::list(&pool, &first, &clock, Locale::Nl)
        .await
        .expect("rolled back opt-in");
    assert_eq!(after_fault.revision, 0);
    assert!(after_fault.items.is_empty());
    pool.execute("DROP FUNCTION undo_restock_insert() CASCADE")
        .await
        .expect("remove post-insert fault");
    let (one, two) = tokio::join!(
        customer_restock::subscribe(&pool, &first, &clock, Locale::Nl, "SET-FILLED", 0),
        customer_restock::subscribe(&pool, &first, &clock, Locale::Nl, "SET-FILLED", 0)
    );
    assert_eq!((one, two), (Ok(1), Ok(1)));
    assert_eq!(
        customer_restock::subscribe(&pool, &first, &clock, Locale::Nl, "SET-FILLED", 0).await,
        Ok(1)
    );
    let stored: (i64,i64) = sqlx::query_as("SELECT generation,opted_in_at FROM customer_restock_subscriptions WHERE account_id=$1 AND sku='SET-FILLED'").bind(id).fetch_one(&pool).await.expect("opt-in");
    assert_eq!(stored, (1, 1100));
    assert_eq!(
        customer_restock::list(&pool, &first, &clock, Locale::Nl)
            .await
            .expect("list")
            .items,
        vec![RestockItem {
            sku: "SET-FILLED".into(),
            publication_revision: 1,
            generation: 1
        }]
    );
    assert!(
        customer_restock::list(&pool, &first, &clock, Locale::De)
            .await
            .expect("no locale fallback")
            .items
            .is_empty()
    );
    assert!(
        customer_restock::list(&pool, &other, &clock, Locale::Nl)
            .await
            .expect("other account")
            .items
            .is_empty()
    );
    assert_eq!(
        customer_restock::unsubscribe(&pool, &other, &clock, "SET-FILLED", 0).await,
        Ok(0)
    );
    assert_eq!(
        customer_restock::unsubscribe(&pool, &first, &clock, "SET-FILLED", 0).await,
        Err(RestockError::Conflict)
    );
    let mut withdrawn = request("withdraw", &PublicationChoice::Withdraw {});
    withdrawn.expected_revision = 1;
    record_publication(&pool, &author(), &clock, withdrawn)
        .await
        .expect("withdraw");
    assert!(
        customer_restock::list(&pool, &first, &clock, Locale::Nl)
            .await
            .expect("hidden")
            .items
            .is_empty()
    );
    assert_eq!(
        customer_restock::unsubscribe(&pool, &first, &clock, "SET-FILLED", 1).await,
        Ok(2)
    );
    assert_eq!(
        customer_restock::unsubscribe(&pool, &first, &clock, "SET-FILLED", 1).await,
        Ok(2)
    );
    let activation = choice();
    let mut republish = request("republish", &activation);
    republish.expected_revision = 2;
    record_publication(&pool, &author(), &clock, republish)
        .await
        .expect("republish");
    assert_eq!(
        customer_restock::subscribe(&pool, &first, &clock, Locale::Nl, "SET-FILLED", 0).await,
        Err(RestockError::Conflict)
    );
    assert_eq!(
        customer_restock::subscribe(&pool, &first, &clock, Locale::Nl, "SET-FILLED", 2).await,
        Ok(3)
    );
    assert_eq!(
        customer_restock::list(&pool, &first, &clock, Locale::Nl)
            .await
            .expect("renewed")
            .items,
        vec![RestockItem {
            sku: "SET-FILLED".into(),
            publication_revision: 3,
            generation: 3
        }]
    );
    pool.execute("CREATE FUNCTION undo_restock_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO customer_restock_subscriptions(account_id,sku,generation,opted_in_at) VALUES (OLD.account_id,OLD.sku,NULL,OLD.opted_in_at); RETURN OLD; END $$; CREATE TRIGGER zzz_undo_restock_delete AFTER DELETE ON customer_restock_subscriptions FOR EACH ROW EXECUTE FUNCTION undo_restock_delete()").await.expect("install isolated post-delete fault");
    assert_eq!(
        customer_restock::clear_all(&pool, &first, &clock, 3).await,
        Err(RestockError::Unavailable)
    );
    let after_fault = customer_restock::list(&pool, &first, &clock, Locale::Nl)
        .await
        .expect("rolled back cancellation");
    assert_eq!(after_fault.revision, 3);
    assert_eq!(after_fault.items[0].generation, 3);
    pool.execute("DROP FUNCTION undo_restock_delete() CASCADE")
        .await
        .expect("remove post-delete fault");
    assert_eq!(
        customer_restock::clear_all(&pool, &first, &clock, 3).await,
        Ok(4)
    );
    assert_eq!(
        customer_restock::subscribe(&pool, &first, &clock, Locale::Nl, "SET-FILLED", 2).await,
        Err(RestockError::Conflict)
    );
    assert_eq!(
        customer_restock::subscribe(&pool, &first, &clock, Locale::Nl, "SET-FILLED", 4).await,
        Ok(5)
    );
    let mut withdrawn_again = request("withdraw-again", &PublicationChoice::Withdraw {});
    withdrawn_again.expected_revision = 3;
    record_publication(&pool, &author(), &clock, withdrawn_again)
        .await
        .expect("withdraw again");
    assert!(
        customer_restock::list(&pool, &first, &clock, Locale::Nl)
            .await
            .expect("hidden again")
            .items
            .is_empty()
    );
    assert_eq!(
        customer_restock::clear_all(&pool, &first, &clock, 5).await,
        Ok(6)
    );
    let after: (i64,i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM stock_balances),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM email_verification_delivery_starts)").fetch_one(&pool).await.expect("unrelated effects");
    assert_eq!(before, after);
    customer_sessions::logout(&pool, &first, &clock)
        .await
        .expect("logout");
    assert_eq!(
        customer_restock::list(&pool, &first, &clock, Locale::Nl).await,
        Err(RestockError::Denied)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_cap_and_generations_serialize_racing_final_subscriptions(pool: PgPool) {
    let _fixture = media_seed(&pool).await;
    let (id, token) = account(&pool, "cap@example.test").await;
    activate(&pool).await;
    for number in 0..26 {
        let sku = format!("SYNTHETIC-{number:02}");
        definition(
            &pool,
            &CatalogueCommand::Variant {
                sku,
                family_code: "ROOM".into(),
                size_code: format!("size {number}"),
                textile_code: "isolated textile".into(),
                fill: Fill::WithFoam,
                lines: lines(2),
            },
            &format!("variant-{number}"),
        )
        .await;
    }
    sqlx::query("INSERT INTO customer_restock_buckets(account_id) VALUES ($1)")
        .bind(id)
        .execute(&pool)
        .await
        .expect("bucket");
    for number in 0..24 {
        sqlx::query("INSERT INTO customer_restock_subscriptions(account_id,sku,generation,opted_in_at) VALUES ($1,$2,NULL,1100)").bind(id).bind(format!("SYNTHETIC-{number:02}")).execute(&pool).await.expect("opt-in");
    }
    let mut held = pool.begin().await.expect("hold bucket");
    sqlx::query("SELECT revision FROM customer_restock_buckets WHERE account_id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *held)
        .await
        .expect("claim");
    let insert = |sku: &'static str| {
        let pool = pool.clone();
        tokio::spawn(async move {
            sqlx::query("INSERT INTO customer_restock_subscriptions(account_id,sku,generation,opted_in_at) VALUES ($1,$2,NULL,1100) /* restock-cap-race */").bind(id).bind(sku).execute(&pool).await
        })
    };
    let a = insert("SYNTHETIC-24");
    let b = insert("SYNTHETIC-25");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%restock-cap-race%'").fetch_one(&pool).await.expect("waiters");
            if count == 2 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.expect("both blocked");
    held.commit().await.expect("release");
    let a = a.await.expect("task a");
    let b = b.await.expect("task b");
    assert_ne!(a.is_ok(), b.is_ok());
    let loser = if a.is_err() { a.err() } else { b.err() }.expect("one capacity failure");
    assert_eq!(
        loser
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514")
    );
    let (count,distinct,revision): (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM customer_restock_subscriptions WHERE account_id=$1),(SELECT count(DISTINCT generation) FROM customer_restock_subscriptions WHERE account_id=$1),revision FROM customer_restock_buckets WHERE account_id=$1").bind(id).fetch_one(&pool).await.expect("bounded state");
    assert_eq!((count, distinct, revision), (25, 25, 25));
    sqlx::query("INSERT INTO customer_restock_subscriptions(account_id,sku,generation,opted_in_at) VALUES ($1,'SYNTHETIC-00',NULL,1100) ON CONFLICT DO NOTHING").bind(id).execute(&pool).await.expect("duplicate at cap");
    assert_eq!(
        customer_restock::subscribe(
            &pool,
            &token,
            &FixedClock(1100),
            Locale::Nl,
            "SET-FILLED",
            25
        )
        .await,
        Err(RestockError::Capacity)
    );
    assert!(
        sqlx::query("UPDATE customer_restock_buckets SET revision=0 WHERE account_id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query(
            "UPDATE customer_restock_subscriptions SET opted_in_at=1200 WHERE account_id=$1"
        )
        .bind(id)
        .execute(&pool)
        .await
        .is_err()
    );
    assert_eq!(
        customer_restock::clear_all(&pool, &token, &FixedClock(1100), 25).await,
        Ok(50)
    );
    let mut repeat = pool.begin().await.expect("repeatable read transaction");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *repeat)
        .await
        .expect("isolation");
    assert!(sqlx::query("INSERT INTO customer_restock_subscriptions(account_id,sku,generation,opted_in_at) VALUES ($1,'SET-FILLED',NULL,1100)").bind(id).execute(&mut *repeat).await.is_err());
    repeat.rollback().await.expect("release repeatable read");
    assert!(sqlx::query("INSERT INTO customer_restock_subscriptions(account_id,sku,generation,opted_in_at) VALUES ($1,'DEMO-COVERS',NULL,1100)").bind(id).execute(&pool).await.is_err());
    assert!(sqlx::query("INSERT INTO customer_restock_subscriptions(account_id,sku,generation,opted_in_at) VALUES ($1,'SET-FILLED',1,1100)").bind(id).execute(&pool).await.is_err());
    assert!(
        sqlx::query("DELETE FROM customer_restock_buckets WHERE account_id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
}

struct MovingClock(std::sync::atomic::AtomicU64);
impl Clock for MovingClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(std::sync::atomic::Ordering::SeqCst))
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_after_real_bucket_wait_denies_stale_cancellation_and_rolls_back(pool: PgPool) {
    let _fixture = media_seed(&pool).await;
    let (id, token) = account(&pool, "expiry@example.test").await;
    let clock = FixedClock(1100);
    activate(&pool).await;
    assert_eq!(
        customer_restock::subscribe(&pool, &token, &clock, Locale::Nl, "SET-FILLED", 0).await,
        Ok(1)
    );
    assert_eq!(
        customer_restock::unsubscribe(&pool, &token, &clock, "SET-FILLED", 0).await,
        Err(RestockError::Conflict)
    );
    let mut held = pool.begin().await.expect("hold bucket");
    sqlx::query("SELECT revision FROM customer_restock_buckets WHERE account_id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *held)
        .await
        .expect("claim");
    let moving = std::sync::Arc::new(MovingClock(std::sync::atomic::AtomicU64::new(1100)));
    let task_clock = moving.clone();
    let task_pool = pool.clone();
    let task_token = CustomerSessionToken::from_bytes(*token.expose_bytes());
    let task = tokio::spawn(async move {
        customer_restock::unsubscribe(&task_pool, &task_token, &*task_clock, "SET-FILLED", 0).await
    });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT revision FROM customer_restock_buckets%'").fetch_one(&pool).await.expect("waiter");
            if count == 1 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("actual lock wait");
    moving.0.store(44300, std::sync::atomic::Ordering::SeqCst);
    held.commit().await.expect("release");
    assert_eq!(task.await.expect("task"), Err(RestockError::Denied));
    // Three live observations precede deletion; expiry at final validation rolls it back.
    let late = StepClock {
        calls: AtomicUsize::new(0),
        after: 3,
        value: Ok(44300),
    };
    assert_eq!(
        customer_restock::unsubscribe(&pool, &token, &late, "SET-FILLED", 1).await,
        Err(RestockError::Denied)
    );
    // Publication sources are claimed after the account bucket. Expiry while
    // waiting for a source must not become a membership-dependent result.
    let mut source = pool.begin().await.expect("hold source");
    sqlx::query("SELECT sku FROM catalogue_variants WHERE sku='SET-FILLED' FOR UPDATE")
        .execute(&mut *source)
        .await
        .expect("claim source");
    let moving = std::sync::Arc::new(MovingClock(std::sync::atomic::AtomicU64::new(1100)));
    let task_clock = moving.clone();
    let task_pool = pool.clone();
    let task_token = CustomerSessionToken::from_bytes(*token.expose_bytes());
    let task = tokio::spawn(async move {
        customer_restock::subscribe(
            &task_pool,
            &task_token,
            &*task_clock,
            Locale::Nl,
            "SET-FILLED",
            1,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT sku FROM catalogue_variants WHERE sku=ANY%'").fetch_one(&pool).await.expect("source waiter");
            if count == 1 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("source lock wait");
    moving.0.store(44300, std::sync::atomic::Ordering::SeqCst);
    source.commit().await.expect("release source");
    assert_eq!(task.await.expect("task"), Err(RestockError::Denied));
    let after = customer_restock::list(&pool, &token, &clock, Locale::Nl)
        .await
        .expect("still opted in");
    assert_eq!(after.revision, 1);
    assert_eq!(after.items.len(), 1);
}

#[sqlx::test(migrations = false)]
async fn upgrade_from_64_preserves_existing_sessions_favourites_and_catalogue(pool: PgPool) {
    MIGRATOR.run_to(64, &pool).await.expect("previous schema");
    let _fixture = media_seed(&pool).await;
    let (id, token) = account(&pool, "upgrade@example.test").await;
    let clock = FixedClock(1100);
    activate(&pool).await;
    let product = commerce_infrastructure::published_catalogue::published_product(
        &pool,
        &clock,
        Locale::Nl,
        "SET-FILLED",
    )
    .await
    .expect("before")
    .expect("product");
    assert_eq!(
        customer_favourites::set(&pool, &token, &clock, Locale::Nl, "SET-FILLED", true, 0).await,
        Ok(1)
    );
    MIGRATOR.run_to(65, &pool).await.expect("upgrade");
    let versions: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE version IN (62,63,64,65) AND success ORDER BY version").fetch_all(&pool).await.expect("migrations");
    assert_eq!(versions, vec![62, 63, 64, 65]);
    let identity = customer_sessions::authenticate(&pool, &token, &clock)
        .await
        .expect("session");
    assert_eq!((identity.account_id, identity.expires_at), (id, 44300));
    assert_eq!(
        commerce_infrastructure::published_catalogue::published_product(
            &pool,
            &clock,
            Locale::Nl,
            "SET-FILLED"
        )
        .await
        .expect("after")
        .expect("product"),
        product
    );
    assert_eq!(
        customer_favourites::list(&pool, &token, &clock, Locale::Nl)
            .await
            .expect("favourites")
            .items
            .len(),
        1
    );
    let empty = customer_restock::list(&pool, &token, &clock, Locale::Nl)
        .await
        .expect("restock list");
    assert_eq!(empty.revision, 0);
    assert!(empty.items.is_empty());
    assert_eq!(
        customer_restock::subscribe(&pool, &token, &clock, Locale::Nl, "SET-FILLED", 0).await,
        Ok(1)
    );
    assert_eq!(
        customer_restock::list(&pool, &token, &clock, Locale::Nl)
            .await
            .expect("restock list")
            .items
            .len(),
        1
    );
    assert_eq!(
        customer_favourites::list(&pool, &token, &clock, Locale::Nl)
            .await
            .expect("favourites unchanged")
            .items
            .len(),
        1
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn opt_in_also_accepts_an_in_stock_variant_without_mutating_stock(pool: PgPool) {
    let _fixture = media_seed(&pool).await;
    let (_, token) = account(&pool, "available@example.test").await;
    let clock = FixedClock(1100);
    activate(&pool).await;
    sqlx::query("INSERT INTO stock_balances(component_code,on_hand,quarantine,reserved) VALUES ('SEAT-COVER',10,0,0),('SEAT-FOAM',10,0,0)")
        .execute(&pool).await.expect("synthetic in-stock balances");
    assert!(
        commerce_infrastructure::published_catalogue::published_product(
            &pool,
            &clock,
            Locale::Nl,
            "SET-FILLED"
        )
        .await
        .expect("projection")
        .expect("product")
        .in_stock
    );
    let before: Vec<(String,i64,i64,i64)> = sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances ORDER BY component_code").fetch_all(&pool).await.expect("balances");
    assert_eq!(
        customer_restock::subscribe(&pool, &token, &clock, Locale::Nl, "SET-FILLED", 0).await,
        Ok(1)
    );
    let after: Vec<(String,i64,i64,i64)> = sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances ORDER BY component_code").fetch_all(&pool).await.expect("balances");
    assert_eq!(before, after);
    let effects: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM stock_movements),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM email_verification_delivery_starts)").fetch_one(&pool).await.expect("unrelated effects");
    assert_eq!(effects, (0, 0, 0));
}
