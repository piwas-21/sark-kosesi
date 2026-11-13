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
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "DEMO-COVERS", true, 0).await,
        Err(FavouriteError::InvalidRequest)
    );
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "SET-FILLED", true, -1).await,
        Err(FavouriteError::InvalidRequest)
    );
    assert_eq!(
        customer_favourites::clear_all(&pool, &first, &clock, -1).await,
        Err(FavouriteError::InvalidRequest)
    );
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
    // The projection is valid while read, but rights can expire after the
    // write. The final check must roll back the membership and revision.
    let expiry_after_write = StepClock {
        calls: AtomicUsize::new(0),
        after: 6,
        value: Ok(2000),
    };
    assert_eq!(
        customer_favourites::set(
            &pool,
            &first,
            &expiry_after_write,
            Locale::Nl,
            "SET-FILLED",
            true,
            0
        )
        .await,
        Err(FavouriteError::Unavailable)
    );
    let after_rights_expiry = customer_favourites::list(&pool, &first, &clock, Locale::Nl)
        .await
        .expect("rights rollback");
    assert_eq!(after_rights_expiry.revision, 0);
    assert!(after_rights_expiry.items.is_empty());
    // An unexpected post-insert database effect is detected before commit.
    pool.execute("CREATE FUNCTION undo_favourite_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN DELETE FROM customer_favourites WHERE account_id=NEW.account_id AND sku=NEW.sku; RETURN NEW; END $$; CREATE TRIGGER zzz_undo_favourite_insert AFTER INSERT ON customer_favourites FOR EACH ROW EXECUTE FUNCTION undo_favourite_insert()").await.expect("install isolated post-insert fault");
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "SET-FILLED", true, 0).await,
        Err(FavouriteError::Unavailable)
    );
    let after_insert_fault = customer_favourites::list(&pool, &first, &clock, Locale::Nl)
        .await
        .expect("read after insert fault");
    assert_eq!(after_insert_fault.revision, 0);
    assert!(after_insert_fault.items.is_empty());
    pool.execute("DROP FUNCTION undo_favourite_insert() CASCADE")
        .await
        .expect("remove isolated post-insert fault");
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
    // A post-delete trigger that silently restores the row is also detected.
    pool.execute("CREATE FUNCTION undo_favourite_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO customer_favourites(account_id,sku) VALUES (OLD.account_id,OLD.sku); RETURN OLD; END $$; CREATE TRIGGER zzz_undo_favourite_delete AFTER DELETE ON customer_favourites FOR EACH ROW EXECUTE FUNCTION undo_favourite_delete()").await.expect("install isolated post-delete fault");
    assert_eq!(
        customer_favourites::clear_all(&pool, &first, &clock, 5).await,
        Err(FavouriteError::Unavailable)
    );
    let after_delete_fault = customer_favourites::list(&pool, &first, &clock, Locale::Nl)
        .await
        .expect("read after delete fault");
    assert_eq!(after_delete_fault.revision, 5);
    assert_eq!(after_delete_fault.items.len(), 1);
    pool.execute("DROP FUNCTION undo_favourite_delete() CASCADE")
        .await
        .expect("remove isolated post-delete fault");
    // A damaged database trigger can suppress a write; neither operation may
    // report success or change the retained membership and revision.
    pool.execute("CREATE FUNCTION suppress_favourite_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$; CREATE TRIGGER aaa_suppress_favourite_delete BEFORE DELETE ON customer_favourites FOR EACH ROW EXECUTE FUNCTION suppress_favourite_delete()")
        .await.expect("install isolated fault");
    assert_eq!(
        customer_favourites::set(&pool, &first, &clock, Locale::Nl, "SET-FILLED", false, 5).await,
        Err(FavouriteError::Unavailable)
    );
    assert_eq!(
        customer_favourites::clear_all(&pool, &first, &clock, 5).await,
        Err(FavouriteError::Unavailable)
    );
    let after_fault = customer_favourites::list(&pool, &first, &clock, Locale::Nl)
        .await
        .expect("read after suppressed writes");
    assert_eq!(after_fault.revision, 5);
    assert_eq!(after_fault.items.len(), 1);
    pool.execute("DROP FUNCTION suppress_favourite_delete() CASCADE")
        .await
        .expect("remove isolated fault");
    // A credential revision change invalidates the previous session even for
    // a read-only empty list; the new session retains access to its own state.
    let proof = customer_passwords::issue(&pool, "other@example.test", &clock)
        .await
        .expect("password proof");
    customer_passwords::replace(
        &pool,
        "other@example.test",
        &proof,
        "another memorable phrase",
        &clock,
    )
    .await
    .expect("password replacement");
    assert_eq!(
        customer_favourites::list(&pool, &other, &clock, Locale::Nl).await,
        Err(FavouriteError::Denied)
    );
    let renewed = admitted_login::login(
        &pool,
        "other@example.test",
        "another memorable phrase",
        &clock,
    )
    .await
    .expect("fresh login");
    assert!(
        customer_favourites::list(&pool, &renewed.token, &clock, Locale::Nl)
            .await
            .expect("fresh session")
            .items
            .is_empty()
    );
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
    let loser_sku = if a.is_err() {
        "SYNTHETIC-24"
    } else {
        "SYNTHETIC-25"
    };
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
    // Simulate pre-existing on-disk damage in this disposable database. The
    // private read and clear must fail closed when native cap custody is absent.
    pool.execute("ALTER TABLE customer_favourites DISABLE TRIGGER guard_favourite_membership")
        .await
        .expect("inject isolated damage");
    sqlx::query("INSERT INTO customer_favourites(account_id,sku) VALUES ($1,$2)")
        .bind(id)
        .bind(loser_sku)
        .execute(&pool)
        .await
        .expect("over-cap damaged row");
    pool.execute("ALTER TABLE customer_favourites ENABLE TRIGGER guard_favourite_membership")
        .await
        .expect("restore guard");
    assert_eq!(
        customer_favourites::list(&pool, &token, &FixedClock(1100), Locale::Nl).await,
        Err(FavouriteError::Unavailable)
    );
    assert_eq!(
        customer_favourites::clear_all(&pool, &token, &FixedClock(1100), 25).await,
        Err(FavouriteError::Unavailable)
    );
    sqlx::query("DELETE FROM customer_favourites WHERE account_id=$1 AND sku=$2")
        .bind(id)
        .bind(loser_sku)
        .execute(&pool)
        .await
        .expect("remove isolated damage");
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
    // A corrupted fixture identity must never cross the private list boundary,
    // even if ordinary native insertion guards have been bypassed.
    pool.execute("ALTER TABLE customer_favourites DISABLE TRIGGER guard_favourite_membership")
        .await
        .expect("inject isolated fixture damage");
    sqlx::query("INSERT INTO customer_favourites(account_id,sku) VALUES ($1,'DEMO-COVERS')")
        .bind(id)
        .execute(&pool)
        .await
        .expect("damaged fixture row");
    pool.execute("ALTER TABLE customer_favourites ENABLE TRIGGER guard_favourite_membership")
        .await
        .expect("restore fixture guard");
    assert_eq!(
        customer_favourites::list(&pool, &token, &FixedClock(1100), Locale::Nl).await,
        Err(FavouriteError::Unavailable)
    );
    assert_eq!(
        customer_favourites::clear_all(&pool, &token, &FixedClock(1100), 0).await,
        Err(FavouriteError::Unavailable)
    );
}

#[sqlx::test(migrations = false)]
async fn upgrade_through_64_preserves_sessions_csrf_favourites_and_grants(pool: PgPool) {
    MIGRATOR.run_to(62, &pool).await.expect("previous schema");
    let _fixture = media_seed(&pool).await;
    let clock = FixedClock(1100);
    approve(&pool).await;
    record_publication(&pool, &author(), &clock, request("activate", &choice()))
        .await
        .expect("activate before upgrade");
    let product = commerce_infrastructure::published_catalogue::published_product(
        &pool,
        &clock,
        Locale::Nl,
        "SET-FILLED",
    )
    .await
    .expect("catalogue before upgrade")
    .expect("published product");
    let (account_id, legacy) = account(&pool, "upgrade@example.test").await;
    let key = commerce_infrastructure::customer_credential_admission::AdmissionKey::new(1, [7; 32])
        .expect("synthetic key");
    let limits =
        commerce_infrastructure::customer_credential_admission::AdmissionLimits::new(1000, 100)
            .expect("synthetic limits");
    let bound = customer_sessions::login_with_csrf(
        &pool,
        "upgrade@example.test",
        "correct horse battery staple",
        &clock,
        &key,
        limits,
    )
    .await
    .expect("bound session before upgrade");
    let pre_upgrade: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM customer_sessions),(SELECT count(*) FROM customer_session_csrf),(SELECT count(*) FROM catalogue_publication_revisions WHERE sku='SET-FILLED' AND locale='nl')")
        .fetch_one(&pool).await.expect("pre-upgrade custody");
    assert_eq!(pre_upgrade, (2, 1, 1));

    MIGRATOR
        .run_to(63, &pool)
        .await
        .expect("upgrade favourites");
    let versions: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE version IN (62,63) AND success ORDER BY version")
        .fetch_all(&pool).await.expect("ordered migrations");
    assert_eq!(versions, vec![62, 63]);
    let preserved: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM customer_sessions),(SELECT count(*) FROM customer_session_csrf),(SELECT count(*) FROM catalogue_publication_revisions WHERE sku='SET-FILLED' AND locale='nl')")
        .fetch_one(&pool).await.expect("preserved custody");
    assert_eq!(preserved, pre_upgrade);
    for token in [&legacy, &bound.session.token] {
        let identity = customer_sessions::authenticate(&pool, token, &clock)
            .await
            .expect("existing session remains live");
        assert_eq!(
            (identity.account_id, identity.expires_at),
            (account_id, 44300)
        );
    }
    let after = commerce_infrastructure::published_catalogue::published_product(
        &pool,
        &clock,
        Locale::Nl,
        "SET-FILLED",
    )
    .await
    .expect("catalogue after upgrade")
    .expect("published product");
    assert_eq!(after, product);
    let empty = customer_favourites::list(&pool, &bound.session.token, &clock, Locale::Nl)
        .await
        .expect("new list");
    assert_eq!(empty.revision, 0);
    assert!(empty.items.is_empty());
    assert_eq!(
        customer_favourites::set(
            &pool,
            &bound.session.token,
            &clock,
            Locale::Nl,
            "SET-FILLED",
            true,
            0
        )
        .await,
        Ok(1)
    );
    assert_eq!(
        customer_favourites::list(&pool, &bound.session.token, &clock, Locale::Nl)
            .await
            .expect("saved favourite")
            .items,
        vec![FavouriteItem {
            sku: "SET-FILLED".into(),
            publication_revision: 1
        }]
    );
    // Upgrade the populated v63 database without seeding operator authority.
    let grants_before: Vec<(String, String)> = sqlx::query_as(
        "SELECT staff_id,capability FROM staff_capabilities ORDER BY staff_id,capability",
    )
    .fetch_all(&pool)
    .await
    .expect("existing staff grants");
    let before_64 = customer_favourites::list(&pool, &bound.session.token, &clock, Locale::Nl)
        .await
        .expect("populated v63 favourites");
    assert_eq!(before_64.revision, 1);
    MIGRATOR
        .run_to(64, &pool)
        .await
        .expect("upgrade review capability");
    let migrations: Vec<(i64, bool, Vec<u8>)> = sqlx::query_as("SELECT version,success,checksum FROM _sqlx_migrations WHERE version IN (62,63,64) ORDER BY version")
        .fetch_all(&pool).await.expect("ordered upgrade custody");
    assert_eq!(migrations.len(), 3);
    for (version, (actual, success, checksum)) in [62_i64, 63, 64].into_iter().zip(migrations) {
        let expected = MIGRATOR
            .iter()
            .find(|migration| migration.version == version)
            .expect("embedded migration");
        assert_eq!(actual, version);
        assert!(
            success && checksum == expected.checksum.as_ref(),
            "exact successful migration checksum"
        );
    }
    let grants_after: Vec<(String, String)> = sqlx::query_as(
        "SELECT staff_id,capability FROM staff_capabilities ORDER BY staff_id,capability",
    )
    .fetch_all(&pool)
    .await
    .expect("preserved staff grants");
    assert_eq!(grants_after, grants_before);
    let seeded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM staff_capabilities WHERE capability='reversal.review'",
    )
    .fetch_one(&pool)
    .await
    .expect("no seeded review grant");
    assert_eq!(seeded, 0);
    sqlx::query(
        "INSERT INTO staff_capabilities(staff_id,capability) VALUES('author','reversal.review')",
    )
    .execute(&pool)
    .await
    .expect("explicit review grant");
    assert!(sqlx::query("INSERT INTO staff_capabilities(staff_id,capability) VALUES('author','reversal.recover')")
        .execute(&pool).await.is_err());
    let after_64 = customer_favourites::list(&pool, &bound.session.token, &clock, Locale::Nl)
        .await
        .expect("favourites after v64");
    assert_eq!(after_64, before_64);
    assert!(
        sqlx::query("UPDATE customer_favourite_buckets SET revision=0 WHERE account_id=$1")
            .bind(account_id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(sqlx::query("UPDATE customer_session_csrf SET csrf_digest=$2 WHERE session_verifier IN (SELECT verifier FROM customer_sessions WHERE account_id=$1)")
        .bind(account_id).bind([0_u8;32].as_slice()).execute(&pool).await.is_err());
    let wrong = commerce_infrastructure::customer_sessions::CustomerCsrfToken::from_bytes([9; 32]);
    assert!(matches!(
        customer_sessions::logout_with_csrf(&pool, &bound.session.token, &wrong, &clock).await,
        Err(commerce_infrastructure::customer_sessions::SessionError::Denied)
    ));
    let identity = customer_sessions::authenticate(&pool, &bound.session.token, &clock)
        .await
        .expect("wrong csrf did not revoke session");
    assert_eq!(identity.account_id, account_id);
    customer_sessions::logout_with_csrf(&pool, &bound.session.token, &bound.csrf, &clock)
        .await
        .expect("existing csrf binding remains usable");
    customer_sessions::authenticate(&pool, &legacy, &clock)
        .await
        .expect("other pre-upgrade session remains live");
}
