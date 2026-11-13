//! Isolated PostgreSQL controls for private favourites and current publication.
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
    customer_favourites::{self, FavouriteError, FavouriteItem},
    customer_sessions::{self, CustomerSessionToken},
};

struct MovingClock(std::sync::atomic::AtomicU64);
impl Clock for MovingClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(std::sync::atomic::Ordering::SeqCst))
    }
}

#[derive(Clone, Copy)]
enum WaitingAction {
    Set { desired: bool, revision: i64 },
    Clear { revision: i64 },
}

async fn expired_while_waiting(
    pool: &PgPool,
    account: i64,
    token: &CustomerSessionToken,
    action: WaitingAction,
    source_lock: bool,
) -> Result<i64, FavouriteError> {
    let mut held = pool.begin().await.expect("hold serialization claim");
    let pattern = if source_lock {
        sqlx::query("SELECT sku FROM catalogue_variants WHERE sku='SET-FILLED' FOR UPDATE")
            .execute(&mut *held)
            .await
            .expect("lock catalogue source");
        "SELECT sku FROM catalogue_variants WHERE sku=ANY%"
    } else {
        sqlx::query(
            "SELECT revision FROM customer_favourite_buckets WHERE account_id=$1 FOR UPDATE",
        )
        .bind(account)
        .execute(&mut *held)
        .await
        .expect("lock account bucket");
        "SELECT revision FROM customer_favourite_buckets%"
    };
    let moving = std::sync::Arc::new(MovingClock(std::sync::atomic::AtomicU64::new(1100)));
    let waiting_clock = moving.clone();
    let waiting_pool = pool.clone();
    let waiting_token = CustomerSessionToken::from_bytes(*token.expose_bytes());
    let waiting = tokio::spawn(async move {
        match action {
            WaitingAction::Set { desired, revision } => {
                customer_favourites::set(
                    &waiting_pool,
                    &waiting_token,
                    &*waiting_clock,
                    Locale::Nl,
                    "SET-FILLED",
                    desired,
                    revision,
                )
                .await
            }
            WaitingAction::Clear { revision } => {
                customer_favourites::clear_all(
                    &waiting_pool,
                    &waiting_token,
                    &*waiting_clock,
                    revision,
                )
                .await
            }
        }
    });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1")
                .bind(pattern).fetch_one(pool).await.expect("observe waiting operation");
            if count == 1 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("operation reached held claim");
    moving.0.store(44300, std::sync::atomic::Ordering::SeqCst);
    held.commit().await.expect("release claim");
    waiting.await.expect("operation task")
}

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

#[sqlx::test(migrator = "MIGRATOR")]
async fn session_owned_versioned_membership_tracks_current_publication(pool: PgPool) {
    let _fixture = media_seed(&pool).await;
    let (first_id, first) = account(&pool, "first@example.test").await;
    let (_, other) = account(&pool, "other@example.test").await;
    let clock = FixedClock(1100);
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "SET-FILLED", true, 0).await,
        Err(FavouriteError::Unavailable)
    );
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "MISSING", true, 0).await,
        Err(FavouriteError::Unavailable)
    );
    approve(&pool).await;
    record_publication(&pool, &author(), &clock, request("activate", &choice()))
        .await
        .expect("activate");
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "SET-FILLED", true, 0).await,
        Ok(1)
    );
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "SET-FILLED", true, 0).await,
        Ok(1)
    );
    let list = customer_favourites::list(&pool, &first, &clock, Locale::Nl)
        .await
        .expect("list");
    assert_eq!(list.revision, 1);
    assert_eq!(
        list.items,
        vec![FavouriteItem {
            sku: "SET-FILLED".into(),
            publication_revision: 1
        }]
    );
    assert!(
        customer_favourites::list(&pool, &other, &clock, Locale::Nl)
            .await
            .expect("other")
            .items
            .is_empty()
    );
    assert!(
        customer_favourites::list(&pool, &first, &clock, Locale::De)
            .await
            .expect("no locale fallback")
            .items
            .is_empty()
    );
    let mut withdrawal = request("withdraw", &PublicationChoice::Withdraw {});
    withdrawal.expected_revision = 1;
    record_publication(&pool, &author(), &clock, withdrawal)
        .await
        .expect("withdraw");
    assert!(
        customer_favourites::list(&pool, &first, &clock, Locale::Nl)
            .await
            .expect("hidden")
            .items
            .is_empty()
    );
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "SET-FILLED", true, 1).await,
        Err(FavouriteError::Unavailable)
    );
    assert_eq!(
        customer_favourites::clear_all(&pool, &first, &clock, 1).await,
        Ok(2)
    );
    let activation = choice();
    let mut republish = request("republish", &activation);
    republish.expected_revision = 2;
    record_publication(&pool, &author(), &clock, republish)
        .await
        .expect("republish");
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "SET-FILLED", true, 0).await,
        Err(FavouriteError::Conflict)
    );
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "SET-FILLED", true, 2).await,
        Ok(3)
    );
    let list = customer_favourites::list(&pool, &first, &clock, Locale::Nl)
        .await
        .expect("republished");
    assert_eq!(list.items[0].publication_revision, 3);
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "SET-FILLED", false, 3).await,
        Ok(4)
    );
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "SET-FILLED", true, 2).await,
        Err(FavouriteError::Conflict)
    );
    assert_eq!(
        customer_favourites::clear_all(&pool, &first, &clock, 0).await,
        Ok(4)
    );
    assert_eq!(
        customer_favourites::list(&pool, &other, &FixedClock(44300), Locale::Nl).await,
        Err(FavouriteError::Denied)
    );
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "SET-FILLED", true, 4).await,
        Ok(5)
    );
    // Stale set and clear operations must not expose the current revision once
    // the customer session expires while waiting for the bucket.
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "SET-FILLED", false, 4).await,
        Err(FavouriteError::Conflict)
    );
    assert_eq!(
        customer_favourites::clear_all(&pool, &first, &clock, 4).await,
        Err(FavouriteError::Conflict)
    );
    for action in [
        WaitingAction::Set {
            desired: false,
            revision: 4,
        },
        WaitingAction::Clear { revision: 4 },
        WaitingAction::Set {
            desired: false,
            revision: 5,
        },
    ] {
        assert_eq!(
            expired_while_waiting(&pool, first_id, &first, action, false).await,
            Err(FavouriteError::Denied)
        );
    }
    // Publication claims can block after the first authority check as well.
    assert_eq!(
        expired_while_waiting(
            &pool,
            first_id,
            &first,
            WaitingAction::Set {
                desired: true,
                revision: 5
            },
            true
        )
        .await,
        Err(FavouriteError::Denied)
    );
    // Expiry after a successful write must still roll that write back.
    let expiry_after_write = StepClock {
        calls: AtomicUsize::new(0),
        after: 3,
        value: Ok(44300),
    };
    assert_eq!(
        customer_favourites::set(
            &pool,
            &first,
            &expiry_after_write,
            Locale::Nl,
            "SET-FILLED",
            false,
            5
        )
        .await,
        Err(FavouriteError::Denied)
    );
    let after_expiry = customer_favourites::list(&pool, &first, &clock, Locale::Nl)
        .await
        .expect("rollback visible");
    assert_eq!(after_expiry.revision, 5);
    assert_eq!(after_expiry.items.len(), 1);
    customer_sessions::logout(&pool, &first, &clock)
        .await
        .expect("logout");
    assert_eq!(
        customer_favourites::list(&pool, &first, &clock, Locale::Nl).await,
        Err(FavouriteError::Denied)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_capacity_serializes_competing_final_inserts_and_rejects_fixture(pool: PgPool) {
    let _fixture = media_seed(&pool).await;
    let (id, token) = account(&pool, "cap@example.test").await;
    // Keep publication rights active beyond session expiry so this test can
    // distinguish an expired session from the otherwise observable capacity.
    rights(
        &pool,
        1,
        &MediaRightsChange::Grant {
            use_scope: MediaUse::ProductPhotography,
            valid_from: 1100,
            term: RightsTerm::Until {
                expires_at: 100_000,
            },
            permission_ref: "isolated long permission".into(),
        },
    )
    .await;
    let mut selection = selected();
    selection.image.rights_revision = 2;
    facts_review(
        &pool,
        selection.facts,
        0,
        FactDecision::Approved,
        "facts-approved",
    )
    .await;
    image_review(
        &pool,
        &selection.image,
        0,
        ImageDecision::Approved,
        "image-approved",
    )
    .await;
    copy_draft(&pool, Locale::Nl, 1, None, 0, "copy-nl").await;
    copy_review(
        &pool,
        Locale::Nl,
        1,
        0,
        TranslationDecision::Approved,
        "copy-nl-approved",
    )
    .await;
    let long_choice = PublicationChoice::Activate { selection };
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("activate", &long_choice),
    )
    .await
    .expect("activate");
    for number in 0..26 {
        let sku = format!("SYNTHETIC-{number:02}");
        definition(
            &pool,
            &CatalogueCommand::Variant {
                sku: sku.clone(),
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
    sqlx::query("INSERT INTO customer_favourite_buckets(account_id) VALUES ($1)")
        .bind(id)
        .execute(&pool)
        .await
        .expect("bucket");
    for number in 0..24 {
        sqlx::query("INSERT INTO customer_favourites(account_id,sku) VALUES ($1,$2)")
            .bind(id)
            .bind(format!("SYNTHETIC-{number:02}"))
            .execute(&pool)
            .await
            .expect("membership");
    }
    let mut held = pool.begin().await.expect("hold bucket");
    sqlx::query("SELECT revision FROM customer_favourite_buckets WHERE account_id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *held)
        .await
        .expect("lock bucket");
    let insert = |sku: &'static str| {
        let pool = pool.clone();
        tokio::spawn(async move {
            sqlx::query("INSERT INTO customer_favourites(account_id,sku) VALUES ($1,$2) /* favourite-cap-race */")
                .bind(id).bind(sku).execute(&pool).await
        })
    };
    let a = insert("SYNTHETIC-24");
    let b = insert("SYNTHETIC-25");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%favourite-cap-race%'")
                .fetch_one(&pool).await.expect("observe waiting writers");
            if waiting == 2 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.expect("both writers reached the held bucket");
    held.commit().await.expect("release bucket");
    let a = a.await.expect("first writer task");
    let b = b.await.expect("second writer task");
    assert_ne!(a.is_ok(), b.is_ok());
    let loser = if a.is_err() { a.err() } else { b.err() }.expect("capacity rejected one writer");
    assert_eq!(
        loser
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514")
    );
    sqlx::query("INSERT INTO customer_favourites(account_id,sku) VALUES ($1,'SYNTHETIC-00') ON CONFLICT DO NOTHING")
        .bind(id).execute(&pool).await.expect("duplicate at cap remains a no-op");

    let (count, revision): (i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM customer_favourites WHERE account_id=$1), revision FROM customer_favourite_buckets WHERE account_id=$1").bind(id).fetch_one(&pool).await.expect("counts");
    assert_eq!((count, revision), (25, 25));
    assert_eq!(
        customer_favourites::set(
            &pool,
            &token,
            &FixedClock(1100),
            Locale::Nl,
            "SET-FILLED",
            true,
            25
        )
        .await,
        Err(FavouriteError::Capacity)
    );
    assert_eq!(
        expired_while_waiting(
            &pool,
            id,
            &token,
            WaitingAction::Set {
                desired: true,
                revision: 25
            },
            false
        )
        .await,
        Err(FavouriteError::Denied)
    );
    assert!(
        sqlx::query("UPDATE customer_favourite_buckets SET revision=0 WHERE account_id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM customer_favourite_buckets WHERE account_id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM customer_favourites WHERE account_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("clear");
    assert!(
        sqlx::query("DELETE FROM customer_favourite_buckets WHERE account_id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO customer_favourites(account_id,sku) VALUES ($1,'DEMO-COVERS')")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
}
