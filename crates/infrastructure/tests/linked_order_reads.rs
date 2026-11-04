//! Private linked-order reads over real accepted orders and live customer sessions.
use commerce_application::Locale;
use commerce_domain::{checkout_quotes::CapturedAmount, publication::PublicationChoice};
use commerce_infrastructure::{
    MIGRATOR,
    checkout_orders::create_order,
    checkout_quotes::issue_quote,
    customer_passwords,
    customer_sessions::{self, CustomerSessionToken},
    email_verification,
    guest_baskets::{GuestCredential, GuestSessionStore},
    guest_order_links::{self, LinkError, LinkedOrderCursor},
    publication::{PublicationRequest, record_publication},
    verified_customer_accounts::establish,
};
use sqlx::PgPool;
use std::{sync::atomic::Ordering, time::Duration};

#[allow(dead_code, unused_imports)]
mod catalogue {
    include!("../../../tests/fixtures/approved_catalogue.rs");
}

#[path = "support/checkout_order_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/checkout_quote_fixture.rs"]
mod quote_fixture;
use quote_fixture::{guest, request, seed, store};

async fn account(
    pool: &PgPool,
    email: &str,
    time: &quote_fixture::Time,
) -> (i64, CustomerSessionToken) {
    let proof = email_verification::issue(pool, email, time)
        .await
        .expect("email proof");
    let id = establish(pool, email, &proof, time)
        .await
        .expect("account")
        .value();
    let proof = customer_passwords::issue(pool, email, time)
        .await
        .expect("password proof");
    customer_passwords::replace(pool, email, &proof, "correct horse battery staple", time)
        .await
        .expect("password setup");
    let token = customer_sessions::login(pool, email, "correct horse battery staple", time)
        .await
        .expect("login")
        .token;
    (id, token)
}

async fn order(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    suffix: &str,
    email: &str,
) -> String {
    let quote = issue_quote(
        store,
        key,
        csrf,
        Locale::Nl,
        &request(&format!("quote-{suffix}")),
    )
    .await
    .expect("quote")
    .snapshot;
    let mut intent = fixture::intent(&quote.binding.quote_id);
    intent.buyer.email = email.into();
    create_order(
        store,
        key,
        csrf,
        &format!("place-{suffix}"),
        &intent,
        &fixture::seller(),
    )
    .await
    .expect("accepted order")
    .snapshot
    .identity
    .id
}

async fn linked(
    store: &GuestSessionStore,
    pool: &PgPool,
    key: &GuestCredential,
    csrf: &str,
    session: &CustomerSessionToken,
    time: &quote_fixture::Time,
    fixture: (&str, &str),
) -> String {
    let id = order(store, key, csrf, fixture.0, fixture.1).await;
    let proof = guest_order_links::issue(store, key, csrf, &id)
        .await
        .expect("link proof");
    guest_order_links::link(pool, session, &id, &proof, time)
        .await
        .expect("linked");
    id
}

fn amount(gross_minor: i64, net_minor: i64, vat_minor: i64) -> CapturedAmount {
    CapturedAmount {
        gross_minor,
        net_minor,
        vat_minor,
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn owner_reads_only_the_captured_customer_projection_and_live_authority(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (guest_session, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&guest_session).expect("guest key");
    let (_, owner) = account(&pool, "buyer@example.test", &time).await;
    let (_, other) = account(&pool, "other@example.test", &time).await;
    let id = linked(
        &store,
        &pool,
        &key,
        &csrf,
        &owner,
        &time,
        ("owned", "buyer@example.test"),
    )
    .await;
    let unlinked = order(&store, &key, &csrf, "unlinked", "buyer@example.test").await;

    let view = guest_order_links::read_linked_order(&pool, &owner, &id, time.as_ref())
        .await
        .expect("owned order");
    assert_eq!(
        (
            &view.order_id,
            view.created_at,
            view.linked_at,
            view.currency.as_str()
        ),
        (&id, 1100, 1100, "EUR")
    );
    assert_eq!(view.lines.len(), 1);
    assert_eq!(
        (
            view.lines[0].sku.as_str(),
            view.lines[0].title.as_str(),
            view.lines[0].quantity
        ),
        ("SET-FILLED", "Isolated acceptance sample", 3)
    );
    assert_eq!(view.lines[0].amounts, amount(37035, 30607, 6428));
    assert_eq!(view.shipping, amount(1210, 1000, 210));
    assert_eq!(view.total, amount(38245, 31607, 6638));
    for (actor, target) in [
        (&other, id.as_str()),
        (&owner, unlinked.as_str()),
        (&owner, "ffffffffffffffffffffffffffffffff"),
    ] {
        assert!(matches!(
            guest_order_links::read_linked_order(&pool, actor, target, time.as_ref()).await,
            Err(LinkError::Denied)
        ));
    }
    let missing = CustomerSessionToken::from_bytes([42; 32]);
    assert!(matches!(
        guest_order_links::read_linked_order(&pool, &missing, &id, time.as_ref()).await,
        Err(LinkError::Denied)
    ));
    assert!(matches!(
        guest_order_links::list_linked_orders(&pool, &missing, None, 2, time.as_ref()).await,
        Err(LinkError::Denied)
    ));
    customer_sessions::logout(&pool, &other, time.as_ref())
        .await
        .expect("logout");
    assert!(matches!(
        guest_order_links::list_linked_orders(&pool, &other, None, 2, time.as_ref()).await,
        Err(LinkError::Denied)
    ));
    let reset = customer_passwords::issue(&pool, "buyer@example.test", time.as_ref())
        .await
        .expect("reset");
    customer_passwords::replace(
        &pool,
        "buyer@example.test",
        &reset,
        "new correct horse battery staple",
        time.as_ref(),
    )
    .await
    .expect("replace");
    assert!(matches!(
        guest_order_links::read_linked_order(&pool, &owner, &id, time.as_ref()).await,
        Err(LinkError::Denied)
    ));
    let current = customer_sessions::login(
        &pool,
        "buyer@example.test",
        "new correct horse battery staple",
        time.as_ref(),
    )
    .await
    .expect("new session");
    assert!(
        guest_order_links::read_linked_order(&pool, &current.token, &id, time.as_ref())
            .await
            .is_ok()
    );
    time.0.store(1100 + 12 * 60 * 60, Ordering::SeqCst);
    assert!(matches!(
        guest_order_links::read_linked_order(&pool, &current.token, &id, time.as_ref()).await,
        Err(LinkError::Denied)
    ));
    assert!(matches!(
        guest_order_links::list_linked_orders(&pool, &current.token, None, 2, time.as_ref()).await,
        Err(LinkError::Denied)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bounded_keyset_pages_keep_same_time_ties_and_exclude_other_accounts(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (guest_session, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&guest_session).expect("guest key");
    let (_, owner) = account(&pool, "buyer@example.test", &time).await;
    let (_, other) = account(&pool, "other@example.test", &time).await;
    let mut expected = Vec::new();
    for i in 0..5 {
        expected.push(
            linked(
                &store,
                &pool,
                &key,
                &csrf,
                &owner,
                &time,
                (&format!("owner-{i}"), "buyer@example.test"),
            )
            .await,
        );
        if i == 2 {
            linked(
                &store,
                &pool,
                &key,
                &csrf,
                &other,
                &time,
                ("foreign", "other@example.test"),
            )
            .await;
        }
    }
    expected.sort_by(|a, b| b.cmp(a));
    for limit in [0, 9, u8::MAX] {
        assert!(matches!(
            guest_order_links::list_linked_orders(&pool, &owner, None, limit, time.as_ref()).await,
            Err(LinkError::Denied)
        ));
    }
    let bad = LinkedOrderCursor {
        linked_at: 1100,
        order_id: "invalid".into(),
    };
    assert!(matches!(
        guest_order_links::list_linked_orders(&pool, &owner, Some(&bad), 2, time.as_ref()).await,
        Err(LinkError::Denied)
    ));
    let mut cursor = None;
    let mut actual = Vec::new();
    for page_number in 0..3 {
        let page =
            guest_order_links::list_linked_orders(&pool, &owner, cursor.as_ref(), 2, time.as_ref())
                .await
                .expect("page");
        assert_eq!(page.orders.len(), if page_number == 2 { 1 } else { 2 });
        assert!(page.orders.iter().all(|view| view.linked_at == 1100));
        actual.extend(page.orders.into_iter().map(|view| view.order_id));
        cursor = page.next_cursor;
        assert_eq!(cursor.is_some(), page_number < 2);
    }
    assert_eq!(actual, expected);
    let foreign = guest_order_links::list_linked_orders(&pool, &other, None, 8, time.as_ref())
        .await
        .expect("foreign page");
    assert_eq!(foreign.orders.len(), 1);
    assert!(!expected.contains(&foreign.orders[0].order_id));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_reciprocal_proof_fails_closed_without_silent_page_omission(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (guest_session, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&guest_session).expect("guest key");
    let (_, owner) = account(&pool, "buyer@example.test", &time).await;
    let good = linked(
        &store,
        &pool,
        &key,
        &csrf,
        &owner,
        &time,
        ("good", "buyer@example.test"),
    )
    .await;
    let damaged = linked(
        &store,
        &pool,
        &key,
        &csrf,
        &owner,
        &time,
        ("damaged", "buyer@example.test"),
    )
    .await;
    assert_eq!(
        guest_order_links::list_linked_orders(&pool, &owner, None, 8, time.as_ref())
            .await
            .expect("healthy page")
            .orders
            .len(),
        2
    );
    // Isolated corruption test only: defeat immutable guards to simulate stored damage.
    let mut tx = pool.begin().await.expect("fixture corruption transaction");
    sqlx::query("ALTER TABLE guest_order_link_proofs DISABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("disable guards");
    sqlx::query("UPDATE guest_order_link_proofs SET consumed_at=consumed_at+1 WHERE order_id=$1")
        .bind(&damaged)
        .execute(&mut *tx)
        .await
        .expect("damage proof");
    sqlx::query("ALTER TABLE guest_order_link_proofs ENABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("restore guards");
    tx.commit().await.expect("commit isolated corruption");
    assert!(
        guest_order_links::read_linked_order(&pool, &owner, &good, time.as_ref())
            .await
            .is_ok()
    );
    assert!(matches!(
        guest_order_links::read_linked_order(&pool, &owner, &damaged, time.as_ref()).await,
        Err(LinkError::Unavailable)
    ));
    assert!(matches!(
        guest_order_links::list_linked_orders(&pool, &owner, None, 8, time.as_ref()).await,
        Err(LinkError::Unavailable)
    ));
}

async fn wait_for_lock(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1")
                .bind(pattern).fetch_one(pool).await.expect("lock observer");
            if waiting > 0 { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("expected lock wait");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn read_holds_session_claim_across_wait_and_rechecks_expiry(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (guest_session, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&guest_session).expect("guest key");
    let (account_id, owner) = account(&pool, "buyer@example.test", &time).await;
    let id = linked(
        &store,
        &pool,
        &key,
        &csrf,
        &owner,
        &time,
        ("wait", "buyer@example.test"),
    )
    .await;
    let expires: i64 =
        sqlx::query_scalar("SELECT expires_at FROM customer_sessions WHERE account_id=$1")
            .bind(account_id)
            .fetch_one(&pool)
            .await
            .expect("expiry");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("LOCK TABLE checkout_orders IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .expect("hold order table");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let worker_token = CustomerSessionToken::from_bytes(*owner.expose_bytes());
    let task = tokio::spawn(async move {
        guest_order_links::read_linked_order(&worker_pool, &worker_token, &id, worker_time.as_ref())
            .await
    });
    wait_for_lock(&pool, "%checkout_orders%").await;
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let worker_token = CustomerSessionToken::from_bytes(*owner.expose_bytes());
    let logout = tokio::spawn(async move {
        customer_sessions::logout(&worker_pool, &worker_token, worker_time.as_ref()).await
    });
    wait_for_lock(
        &pool,
        "SELECT verified_at FROM verified_customer_accounts WHERE id=$1 FOR UPDATE%",
    )
    .await;
    assert!(!task.is_finished() && !logout.is_finished());
    time.0.store(expires as u64, Ordering::SeqCst);
    blocker.rollback().await.expect("release table");
    assert!(matches!(
        task.await.expect("read task"),
        Err(LinkError::Denied)
    ));
    // Logout waits for the held claim; expiry makes its own authority invalid too.
    assert!(logout.await.expect("logout task").is_err());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn historical_title_survives_withdrawal_but_altered_snapshot_fails_closed(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (guest_session, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&guest_session).expect("guest key");
    let (_, owner) = account(&pool, "buyer@example.test", &time).await;
    let good = linked(
        &store,
        &pool,
        &key,
        &csrf,
        &owner,
        &time,
        ("title-good", "buyer@example.test"),
    )
    .await;
    let damaged = linked(
        &store,
        &pool,
        &key,
        &csrf,
        &owner,
        &time,
        ("title-damaged", "buyer@example.test"),
    )
    .await;
    for id in [&good, &damaged] {
        let view = guest_order_links::read_linked_order(&pool, &owner, id, time.as_ref())
            .await
            .expect("original historical title");
        assert_eq!(view.lines[0].title, "Isolated acceptance sample");
    }

    let receipt = record_publication(
        &pool,
        &catalogue::author(),
        time.as_ref(),
        PublicationRequest {
            sku: "SET-FILLED",
            locale: Locale::Nl,
            expected_revision: 1,
            effect_key: "withdraw-after-order",
            choice: &PublicationChoice::Withdraw {},
            evidence_ref: "synthetic withdrawal",
            reason: "synthetic withdrawal",
            confirmed: true,
        },
    )
    .await
    .expect("withdraw current publication");
    assert_eq!(receipt.revision, 2);
    for id in [&good, &damaged] {
        let view = guest_order_links::read_linked_order(&pool, &owner, id, time.as_ref())
            .await
            .expect("accepted copy remains readable after withdrawal");
        assert_eq!(view.lines[0].title, "Isolated acceptance sample");
    }

    // Isolated corruption bypasses the immutable guard; this title remains valid text.
    let mut tx = pool.begin().await.expect("fixture corruption transaction");
    sqlx::query("ALTER TABLE checkout_orders DISABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("disable guards");
    sqlx::query("UPDATE checkout_orders SET snapshot=jsonb_set(snapshot,'{products,0,title}',to_jsonb('Plausible altered title'::text)) WHERE id=$1")
        .bind(&damaged).execute(&mut *tx).await.expect("damage title");
    sqlx::query("ALTER TABLE checkout_orders ENABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("restore guards");
    tx.commit().await.expect("commit isolated corruption");
    let view = guest_order_links::read_linked_order(&pool, &owner, &good, time.as_ref())
        .await
        .expect("healthy historical control");
    assert_eq!(view.lines[0].title, "Isolated acceptance sample");
    assert!(matches!(
        guest_order_links::read_linked_order(&pool, &owner, &damaged, time.as_ref()).await,
        Err(LinkError::Unavailable)
    ));
    assert!(matches!(
        guest_order_links::list_linked_orders(&pool, &owner, None, 8, time.as_ref()).await,
        Err(LinkError::Unavailable)
    ));
}

async fn set_german_source_review(pool: &PgPool, revision: i32) {
    let mut tx = pool.begin().await.expect("isolated publication mutation");
    sqlx::query("ALTER TABLE catalogue_publication_revisions DISABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("disable publication guard");
    sqlx::query("UPDATE catalogue_publication_revisions SET choice=jsonb_set(choice,'{selection,source_review_revision}',to_jsonb($1::integer)) WHERE sku='SET-FILLED' AND locale='de' AND revision=1")
        .bind(revision).execute(&mut *tx).await.expect("change selected source review");
    sqlx::query("ALTER TABLE catalogue_publication_revisions ENABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("restore publication guard");
    tx.commit().await.expect("commit isolated mutation");
}

async fn set_dutch_first_review_decision(pool: &PgPool, decision: &str) {
    let mut tx = pool.begin().await.expect("isolated review mutation");
    sqlx::query("ALTER TABLE catalogue_translation_reviews DISABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("disable review guard");
    sqlx::query("UPDATE catalogue_translation_reviews SET decision=$1 WHERE sku='SET-FILLED' AND locale='nl' AND translation_revision=1 AND revision=1")
        .bind(decision).execute(&mut *tx).await.expect("change selected source decision");
    sqlx::query("ALTER TABLE catalogue_translation_reviews ENABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("restore review guard");
    tx.commit().await.expect("commit isolated mutation");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn translated_history_requires_its_exact_approved_dutch_source_review(pool: PgPool) {
    use commerce_domain::translations::TranslationDecision;
    use commerce_infrastructure::translations::{TranslationRequest, record_translation};

    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let mut german_copy = catalogue::copy();
    german_copy.title = "Synthetisches Beispielsitzset".into();
    record_translation(
        &pool,
        &catalogue::author(),
        time.as_ref(),
        TranslationRequest {
            sku: "SET-FILLED",
            locale: Locale::De,
            specification_revision: 1,
            source_revision: Some(1),
            expected_revision: 0,
            effect_key: "copy-de",
            draft: &german_copy,
            reason: "synthetic German copy",
            confirmed: true,
        },
    )
    .await
    .expect("German copy draft");
    catalogue::copy_review(
        &pool,
        Locale::De,
        1,
        0,
        TranslationDecision::Approved,
        "copy-de-approved",
    )
    .await;
    let mut selected = catalogue::selected();
    selected.source_review_revision = Some(1);
    let choice = PublicationChoice::Activate {
        selection: selected,
    };
    let mut publication = catalogue::request("activate-de", &choice);
    publication.locale = Locale::De;
    record_publication(&pool, &catalogue::author(), time.as_ref(), publication)
        .await
        .expect("approved German publication");

    let (guest_session, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&guest_session).expect("guest key");
    let (_, owner) = account(&pool, "buyer@example.test", &time).await;
    let quote = issue_quote(&store, &key, &csrf, Locale::De, &request("quote-de"))
        .await
        .expect("German quote")
        .snapshot;
    let mut seller = fixture::seller();
    seller.locale = "de".into();
    let id = create_order(
        &store,
        &key,
        &csrf,
        "place-de",
        &fixture::intent(&quote.binding.quote_id),
        &seller,
    )
    .await
    .expect("German accepted order")
    .snapshot
    .identity
    .id;
    let proof = guest_order_links::issue(&store, &key, &csrf, &id)
        .await
        .expect("link proof");
    guest_order_links::link(&pool, &owner, &id, &proof, time.as_ref())
        .await
        .expect("link");
    let view = guest_order_links::read_linked_order(&pool, &owner, &id, time.as_ref())
        .await
        .expect("healthy translated history");
    assert_eq!(view.lines[0].title, "Synthetisches Beispielsitzset");

    // A newer source and its two real reviews do not alter the captured source.
    catalogue::copy_draft(&pool, Locale::Nl, 1, None, 1, "copy-nl-new").await;
    catalogue::copy_review(
        &pool,
        Locale::Nl,
        2,
        0,
        TranslationDecision::Approved,
        "copy-nl-new-review-1",
    )
    .await;
    catalogue::copy_review(
        &pool,
        Locale::Nl,
        2,
        1,
        TranslationDecision::Approved,
        "copy-nl-new-review-2",
    )
    .await;
    assert!(
        guest_order_links::read_linked_order(&pool, &owner, &id, time.as_ref())
            .await
            .is_ok()
    );

    // Review 2 exists for Dutch source 2, but not the source 1 bound to German copy 1.
    for revision in [99, 2] {
        set_german_source_review(&pool, revision).await;
        assert!(matches!(
            guest_order_links::read_linked_order(&pool, &owner, &id, time.as_ref()).await,
            Err(LinkError::Unavailable)
        ));
        assert!(matches!(
            guest_order_links::list_linked_orders(&pool, &owner, None, 8, time.as_ref()).await,
            Err(LinkError::Unavailable)
        ));
        set_german_source_review(&pool, 1).await;
        assert!(
            guest_order_links::read_linked_order(&pool, &owner, &id, time.as_ref())
                .await
                .is_ok()
        );
    }
    set_dutch_first_review_decision(&pool, "rejected").await;
    assert!(matches!(
        guest_order_links::read_linked_order(&pool, &owner, &id, time.as_ref()).await,
        Err(LinkError::Unavailable)
    ));
    set_dutch_first_review_decision(&pool, "approved").await;
    assert!(
        guest_order_links::read_linked_order(&pool, &owner, &id, time.as_ref())
            .await
            .is_ok()
    );
}
