//! Fully accepted synthetic catalogue/order/payment custody, never merchant evidence.
use super::*;
use commerce_application::Locale;
#[allow(dead_code, unused_imports)]
mod catalogue {
    include!("../../../../tests/fixtures/approved_catalogue.rs");
}
use crate::payment_test_fixture::{order, promotion, quote};
use commerce_domain::{
    guest_baskets::BasketUpdate,
    inventory::WarehouseChange,
    quotes::{BasketLine, ProductRule},
};
use commerce_infrastructure::{
    checkout_orders::create_order,
    checkout_quotes::{issue_promoted_quote, issue_quote},
    guest_baskets::*,
    stock::{WarehouseRequest, warehouse_movement},
};

static NEXT_IDS: AtomicU64 = AtomicU64::new(60_000);

pub struct Fixture {
    pub time: Arc<quote::Time>,
    pub order: commerce_domain::checkout_orders::OrderSnapshot,
    pub frozen: FrozenAttempt,
    pub ids: Arc<crate::payment_test_fixture::Ids>,
}
pub async fn receive(pool: &PgPool, key: &str, component: &str, change: WarehouseChange) {
    warehouse_movement(
        pool,
        WarehouseRequest {
            effect_key: key,
            component_code: component,
            change,
            actor: "fixture-warehouse",
            reason: "synthetic physical custody",
        },
    )
    .await
    .expect("genuine warehouse movement");
}
pub(crate) async fn additional_variant(
    pool: &PgPool,
    sku: &str,
    filled: bool,
    components: &[(String, u32)],
) {
    use catalogue::*;
    definition(
        pool,
        &CatalogueCommand::Variant {
            sku: sku.into(),
            family_code: "ROOM".into(),
            size_code: sku.into(),
            textile_code: "settlement textile".into(),
            fill: if filled {
                Fill::WithFoam
            } else {
                Fill::CoversOnly
            },
            lines: components
                .iter()
                .map(|(code, quantity)| BomLineDraft {
                    component_code: code.clone(),
                    quantity: *quantity,
                })
                .collect(),
        },
        sku,
    )
    .await;
    let mut specification = draft(filled);
    let cover = specification.components[0].clone();
    let foam = if filled {
        Some(specification.components[1].clone())
    } else {
        None
    };
    specification.components = components
        .iter()
        .map(|(code, _)| {
            let mut value = if code.contains("FOAM") {
                foam.clone().expect("filled foam material")
            } else {
                cover.clone()
            };
            value.component_code = code.clone();
            value
        })
        .collect();
    record_specification(
        pool,
        &author(),
        &FixedClock(1100),
        SpecificationRequest {
            sku,
            bom_revision: 1,
            expected_revision: 0,
            effect_key: &format!("settlement-specification-{sku}"),
            draft: &specification,
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("accepted specification");
    record_price_revision(
        pool,
        &author(),
        &FixedClock(1100),
        PriceRevisionRequest {
            sku,
            expected_revision: 0,
            effect_key: &format!("settlement-price-{sku}"),
            price: commerce_domain::Money::eur(12345).expect("literal price"),
            evidence_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("accepted price");
    record_factual_review(
        pool,
        &reviewer(),
        &FixedClock(1100),
        FactReviewRequest {
            sku,
            target: selected().facts,
            expected_review_revision: 0,
            effect_key: &format!("settlement-facts-{sku}"),
            decision: FactDecision::Approved,
            evidence_ref: "synthetic",
            qualification_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("independent factual review");
    record_image_review(
        pool,
        &reviewer(),
        &FixedClock(1100),
        ImageReviewRequest {
            sku,
            target: &selected().image,
            expected_revision: 0,
            effect_key: &format!("settlement-image-{sku}"),
            decision: ImageDecision::Approved,
            evidence_ref: "synthetic",
            qualification_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("independent image review");
    record_translation(
        pool,
        &author(),
        &FixedClock(1100),
        TranslationRequest {
            sku,
            locale: Locale::Nl,
            specification_revision: 1,
            source_revision: None,
            expected_revision: 0,
            effect_key: &format!("settlement-copy-{sku}"),
            draft: &copy(),
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("copy");
    record_review(
        pool,
        &reviewer(),
        &FixedClock(1100),
        TranslationReviewRequest {
            sku,
            locale: Locale::Nl,
            translation_revision: 1,
            expected_review_revision: 0,
            effect_key: &format!("settlement-copy-review-{sku}"),
            decision: TranslationDecision::Approved,
            evidence_ref: "synthetic",
            qualification_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("independent copy review");
    publish(pool, sku).await;
}
async fn publish(pool: &PgPool, sku: &str) {
    use catalogue::*;
    record_publication(
        pool,
        &author(),
        &FixedClock(1100),
        PublicationRequest {
            sku,
            locale: Locale::Nl,
            expected_revision: 0,
            effect_key: &format!("settlement-publication-{sku}"),
            choice: &choice(),
            evidence_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("explicit approved publication");
}
pub async fn seed(pool: &PgPool) {
    let _guard = catalogue::media_seed(pool).await;
    catalogue::approve(pool).await;
    publish(pool, "SET-FILLED").await;
    additional_variant(pool, "SET-COVERS", false, &[("SEAT-COVER".into(), 2)]).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','market.manage'),('reviewer','market.review')").await.expect("isolated market grants");
    let mut policy = quote::policy();
    policy.products.push(ProductRule {
        sku: "SET-COVERS".into(),
        publication_revision: 1,
        max_quantity: 10,
        vat_basis_points: 2100,
    });
    quote::draft(pool, &policy).await;
    quote::decide(pool, 1, 0, true, "settlement-market").await;
    receive(
        pool,
        "initial-cover",
        "SEAT-COVER",
        WarehouseChange::Receive(10),
    )
    .await;
    receive(
        pool,
        "initial-foam",
        "SEAT-FOAM",
        WarehouseChange::Receive(4),
    )
    .await;
    receive(
        pool,
        "quarantined-cover",
        "SEAT-COVER",
        WarehouseChange::Quarantine(1),
    )
    .await;
    receive(
        pool,
        "quarantined-foam",
        "SEAT-FOAM",
        WarehouseChange::Quarantine(1),
    )
    .await;
}
pub async fn accepted(pool: &PgPool, only_covers: bool, paid: bool) -> Fixture {
    seed(pool).await;
    let (store, time) = quote::store(pool, 1100);
    accepted_with_store(pool, store, time, only_covers, paid).await
}
pub async fn accepted_with_store(
    pool: &PgPool,
    store: GuestSessionStore,
    time: Arc<quote::Time>,
    only_covers: bool,
    paid: bool,
) -> Fixture {
    accept_order(pool, store, time, only_covers, paid, false).await
}
pub async fn accepted_with_promotion(pool: &PgPool) -> Fixture {
    seed(pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')")
        .await
        .expect("isolated promotion grants");
    promotion::draft(pool, &promotion::policy(), "settlement-promotion-draft")
        .await
        .expect("promotion draft");
    promotion::decide(pool, "settlement-promotion-review", 1, 0, true)
        .await
        .expect("independent promotion approval");
    let (store, time) = quote::store(pool, 1100);
    accept_order(pool, store, time, false, true, true).await
}
async fn accept_order(
    pool: &PgPool,
    store: GuestSessionStore,
    time: Arc<quote::Time>,
    only_covers: bool,
    paid: bool,
    promoted: bool,
) -> Fixture {
    let session = tower_sessions::Session::new(None, Arc::new(store.clone()), None);
    let established = ensure_session(&store, &session).await.expect("guest");
    let csrf = established.csrf_token().to_owned();
    let owner = GuestCredential::from_session(&session).expect("owner");
    let mut lines = vec![BasketLine {
        sku: "SET-COVERS".into(),
        quantity: 1,
    }];
    if !only_covers {
        lines.push(BasketLine {
            sku: "SET-FILLED".into(),
            quantity: 1,
        });
    }
    write_basket(
        &store,
        &owner,
        &csrf,
        &BasketUpdate {
            expected_revision: 0,
            effect_key: "settlement-basket".into(),
            lines,
        },
    )
    .await
    .expect("exact two-member basket");
    let request = quote::request("settlement-quote");
    let quote_id = if promoted {
        issue_promoted_quote(&store, &owner, &csrf, Locale::Nl, &request, "COURTYARD")
            .await
            .expect("owned promoted quote")
            .snapshot
            .original
            .binding
            .quote_id
    } else {
        issue_quote(&store, &owner, &csrf, Locale::Nl, &request)
            .await
            .expect("owned accepted quote")
            .snapshot
            .binding
            .quote_id
    };
    let order = create_order(
        &store,
        &owner,
        &csrf,
        "settlement-order",
        &order::intent(&quote_id),
        &order::seller(),
    )
    .await
    .expect("genuine immutable order and original group")
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
        "settlement-attempt",
        &crate::payment_test_fixture::intent(&order),
        &configuration,
    )
    .await
    .expect("original payment attempt")
    .frozen;
    start_dispatch(
        pool,
        time.as_ref(),
        ids.as_ref(),
        &frozen.snapshot().binding.id,
        &configuration,
    )
    .await
    .expect("original dispatch");
    let fixture = Fixture {
        time,
        order,
        frozen,
        ids,
    };
    if paid {
        observe(pool, &fixture, "tr_SETTLEMENT", "paid", "paid").await;
        recognize(
            pool,
            fixture.time.as_ref(),
            fixture.ids.as_ref(),
            &fixture.frozen.snapshot().binding.id,
        )
        .await
        .expect("received recognition");
    }
    fixture
}
pub async fn verified(frozen: &FrozenAttempt, id: &str, status: &str) -> VerifiedPayment {
    let mut response = payment(frozen.snapshot());
    response["id"] = id.into();
    response["status"] = status.into();
    let amount = frozen.snapshot().binding.amount_minor;
    response["amount"]["value"] = format!("{}.{:02}", amount / 100, amount % 100).into();
    let server = Server::start(vec![profile(), Reply::Json(200, response)]);
    local(&server)
        .verify_profile()
        .await
        .expect("authenticated profile")
        .fetch(frozen.snapshot(), id)
        .await
        .expect("separately authenticated exact original amount fetch")
}
pub async fn observe(pool: &PgPool, f: &Fixture, id: &str, status: &str, effect: &str) {
    record_verified(
        pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
        effect,
        &verified(&f.frozen, id, status).await,
    )
    .await
    .expect("immutable authenticated observation");
    resolve(
        pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
        effect,
    )
    .await
    .expect("complete identity resolution");
}
pub async fn balances(pool: &PgPool) -> Vec<(String, i64, i64, i64)> {
    sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances ORDER BY component_code").fetch_all(pool).await.expect("literal physical component balances")
}
pub async fn counts(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM order_stock_settlements),(SELECT count(*) FROM commerce_outbox WHERE kind='order.stock_settled.v1'),(SELECT count(*) FROM stock_movements WHERE action='consume-hold'),(SELECT count(*) FROM basket_hold_groups),(SELECT count(*) FROM stock_holds)").fetch_one(pool).await.expect("canonical settlement effects")
}
pub async fn unchanged(pool: &PgPool) -> Value {
    // Old rows lack the new promotion marker. A SQL null adds no custody.
    sqlx::query_scalar("SELECT jsonb_build_object('orders',(SELECT jsonb_agg(CASE WHEN to_jsonb(o)->'promotion_redemption_transaction_id'='null'::jsonb THEN to_jsonb(o)-'promotion_redemption_transaction_id' ELSE to_jsonb(o) END ORDER BY id) FROM checkout_orders o),'attempts',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM payment_attempts a),'groups',(SELECT jsonb_agg(to_jsonb(g) ORDER BY id) FROM basket_hold_groups g),'members',(SELECT jsonb_agg(to_jsonb(m) ORDER BY hold_id) FROM basket_hold_members m),'source',(SELECT jsonb_agg(to_jsonb(l) ORDER BY hold_id,component_code) FROM stock_hold_lines l))").fetch_one(pool).await.expect("original source snapshots")
}
pub async fn footprint(pool: &PgPool) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object('balances',(SELECT jsonb_agg(to_jsonb(b) ORDER BY component_code) FROM stock_balances b),'holds',(SELECT jsonb_agg(to_jsonb(h) ORDER BY id) FROM stock_holds h),'moves',(SELECT jsonb_agg(to_jsonb(m) ORDER BY sequence) FROM stock_movements m),'settlements',(SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM order_stock_settlements s),'events',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM commerce_outbox e WHERE kind='order.stock_settled.v1'))").fetch_one(pool).await.expect("all atomic physical effects")
}

#[path = "order_stock_bounds.rs"]
pub(crate) mod bounds;
