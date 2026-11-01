//! Real accepted orders exercise multiple codes per kind and the full pair bound.
//! This file is a child of order_stock_fixture.rs; no raw accepted snapshots.
use super::*;
use commerce_domain::{ComponentKind, catalogue_authoring::CatalogueCommand};

async fn component(pool: &PgPool, code: &str, kind: ComponentKind, on_hand: u32) {
    catalogue::definition(
        pool,
        &CatalogueCommand::Component {
            code: code.into(),
            component_kind: kind,
        },
        code,
    )
    .await;
    receive(pool, code, code, WarehouseChange::Receive(on_hand)).await;
}

async fn market(pool: &PgPool, skus: &[String]) {
    let mut policy = quote::policy();
    policy.revision = 2;
    policy.products = skus
        .iter()
        .map(|sku| ProductRule {
            sku: sku.clone(),
            publication_revision: 1,
            max_quantity: 1,
            vat_basis_points: 2100,
        })
        .collect();
    policy.shipping.max_parcels = 20;
    policy.shipping.max_total_weight_g = 60000;
    quote::draft(pool, &policy).await;
    quote::decide(pool, 2, 0, true, "bounded-settlement-market").await;
}

async fn accepted_lines(pool: &PgPool, skus: &[String]) -> Fixture {
    let (store, time) = quote::store(pool, 1100);
    let session = tower_sessions::Session::new(None, Arc::new(store.clone()), None);
    let established = ensure_session(&store, &session).await.expect("owned guest");
    let csrf = established.csrf_token().to_owned();
    let owner = GuestCredential::from_session(&session).expect("guest identity");
    write_basket(
        &store,
        &owner,
        &csrf,
        &BasketUpdate {
            expected_revision: 0,
            effect_key: "bounded-basket".into(),
            lines: skus
                .iter()
                .map(|sku| BasketLine {
                    sku: sku.clone(),
                    quantity: 1,
                })
                .collect(),
        },
    )
    .await
    .expect("genuine bounded basket");
    let quoted = issue_quote(
        &store,
        &owner,
        &csrf,
        Locale::Nl,
        &quote::request("bounded-quote"),
    )
    .await
    .expect("approved physical catalogue quote")
    .snapshot;
    let order = create_order(
        &store,
        &owner,
        &csrf,
        "bounded-order",
        &order::intent(&quoted.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("accepted order and complete original reservation")
    .snapshot;
    time.0.store(1500, Ordering::SeqCst);
    let ids = Arc::new(crate::payment_test_fixture::Ids(AtomicU64::new(
        NEXT_IDS.fetch_add(10_000, Ordering::SeqCst),
    )));
    let configuration = config(&credential());
    let frozen = prepare_attempt(
        &store,
        &owner,
        &csrf,
        "bounded-payment",
        &crate::payment_test_fixture::intent(&order),
        &configuration,
    )
    .await
    .expect("original captured payment")
    .frozen;
    start_dispatch(
        pool,
        time.as_ref(),
        ids.as_ref(),
        &frozen.snapshot().binding.id,
        &configuration,
    )
    .await
    .expect("original dispatch custody");
    let fixture = Fixture {
        time,
        order,
        frozen,
        ids,
    };
    observe(
        pool,
        &fixture,
        "tr_BOUNDED_SETTLEMENT",
        "paid",
        "bounded-paid",
    )
    .await;
    recognize(
        pool,
        fixture.time.as_ref(),
        fixture.ids.as_ref(),
        &fixture.frozen.snapshot().binding.id,
    )
    .await
    .expect("authenticated received recognition");
    fixture
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_settlement_full_twenty_by_sixty_four_source_preserves_all_1280_member_pairs(
    pool: PgPool,
) {
    seed(&pool).await;
    let mut components = Vec::new();
    for kind in [ComponentKind::Cover, ComponentKind::Foam] {
        for index in 0..32 {
            let label = match kind {
                ComponentKind::Cover => "COVER",
                ComponentKind::Foam => "FOAM",
            };
            let code = format!("BOUND-{label}-{index:02}");
            component(&pool, &code, kind, 40).await;
            components.push((code, 1));
        }
    }
    let skus: Vec<_> = (0..20)
        .map(|index| format!("BOUND-SET-{index:02}"))
        .collect();
    for sku in &skus {
        additional_variant(&pool, sku, true, &components).await;
    }
    market(&pool, &skus).await;
    let fixture = accepted_lines(&pool, &skus).await;
    assert_eq!(fixture.order.quote.total.gross_minor, 248110);
    let source = unchanged(&pool).await;
    let before: Vec<(String, i64, i64, i64)> = sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code LIKE 'BOUND-%' ORDER BY component_code")
        .fetch_all(&pool).await.expect("all 64 physical component balances");
    assert_eq!(before.len(), 64);
    assert!(
        before
            .iter()
            .all(|row| (row.1, row.2, row.3) == (40, 0, 20))
    );
    let untouched: Vec<(String, i64, i64, i64)> = sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code NOT LIKE 'BOUND-%' ORDER BY component_code")
        .fetch_all(&pool).await.expect("complete other physical stock");
    assert_eq!(
        untouched,
        vec![
            ("SEAT-COVER".into(), 10, 1, 0),
            ("SEAT-FOAM".into(), 4, 1, 0)
        ]
    );
    let reserve_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM stock_movements WHERE action='reserve'")
            .fetch_one(&pool)
            .await
            .expect("all original reserve pairs");
    assert_eq!(reserve_count, 1280);

    let receipt = settle(
        &pool,
        fixture.time.as_ref(),
        fixture.ids.as_ref(),
        &fixture.order.identity.id,
    )
    .await
    .expect("complete maximum source")
    .settlement;
    assert_eq!(receipt.footprint.members.len(), 20);
    assert!(
        receipt
            .footprint
            .members
            .iter()
            .all(|member| member.components.len() == 64)
    );
    assert_eq!(
        receipt
            .footprint
            .members
            .iter()
            .map(|member| member.components.len())
            .sum::<usize>(),
        1280
    );
    assert_eq!(receipt.footprint.components.len(), 64);
    assert_eq!(counts(&pool).await, (1, 1, 1280, 1, 20));
    let consumed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM stock_holds WHERE state='consumed'")
            .fetch_one(&pool)
            .await
            .expect("all original holds consumed");
    assert_eq!(consumed, 20);
    let after: Vec<(String, i64, i64, i64)> = sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code LIKE 'BOUND-%' ORDER BY component_code")
        .fetch_all(&pool).await.expect("all post-settlement physical balances");
    assert_eq!(after.len(), 64);
    assert!(after.iter().all(|row| (row.1, row.2, row.3) == (20, 0, 0)));
    let other_after: Vec<(String, i64, i64, i64)> = sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code NOT LIKE 'BOUND-%' ORDER BY component_code")
        .fetch_all(&pool).await.expect("all other physical stock remains unchanged");
    assert_eq!(other_after, untouched);
    assert_eq!(unchanged(&pool).await, source);
    assert!(
        read_settlement(&pool, &fixture.order.identity.id)
            .await
            .expect("bounded full historical read")
            == Some(receipt)
    );
    assert_eq!(counts(&pool).await, (1, 1, 1280, 1, 20));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_settlement_covers_only_keeps_each_cover_code_separate_and_has_no_foam_effect(
    pool: PgPool,
) {
    seed(&pool).await;
    let components = vec![
        ("BOUND-COVER-A".to_owned(), 1),
        ("BOUND-COVER-B".to_owned(), 2),
        ("BOUND-COVER-C".to_owned(), 3),
    ];
    for (code, _) in &components {
        component(&pool, code, ComponentKind::Cover, 10).await;
    }
    let skus = vec!["BOUND-COVERS".to_owned()];
    additional_variant(&pool, &skus[0], false, &components).await;
    market(&pool, &skus).await;
    let fixture = accepted_lines(&pool, &skus).await;
    assert_eq!(fixture.order.quote.total.gross_minor, 13555);
    let source = unchanged(&pool).await;
    let receipt = settle(
        &pool,
        fixture.time.as_ref(),
        fixture.ids.as_ref(),
        &fixture.order.identity.id,
    )
    .await
    .expect("all distinct cover components")
    .settlement;
    assert_eq!(counts(&pool).await, (1, 1, 3, 1, 1));
    let after: Vec<(String, i64, i64, i64)> = sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code LIKE 'BOUND-%' ORDER BY component_code")
        .fetch_all(&pool).await.expect("distinct covers remain distinct physical pools");
    assert_eq!(
        after,
        vec![
            ("BOUND-COVER-A".into(), 9, 0, 0),
            ("BOUND-COVER-B".into(), 8, 0, 0),
            ("BOUND-COVER-C".into(), 7, 0, 0)
        ]
    );
    let foam: (i64, i64, i64) = sqlx::query_as(
        "SELECT on_hand,quarantine,reserved FROM stock_balances WHERE component_code='SEAT-FOAM'",
    )
    .fetch_one(&pool)
    .await
    .expect("separate untouched foam");
    assert_eq!(foam, (4, 1, 0));
    let foam_effects: i64 = sqlx::query_scalar("SELECT count(*) FROM stock_movements m JOIN physical_components c ON c.code=m.component_code WHERE c.kind='foam' AND m.action IN ('reserve','consume-hold')")
        .fetch_one(&pool).await.expect("zero foam stock effects");
    assert_eq!(foam_effects, 0);
    assert_eq!(unchanged(&pool).await, source);
    assert!(
        read_settlement(&pool, &fixture.order.identity.id)
            .await
            .expect("exact distinct-cover history")
            == Some(receipt)
    );
}
