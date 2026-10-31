use super::infrastructure::{
    catalogue_authoring::{DefinitionRequest, record_definition},
    staff_access::SessionToken,
    stock::{WarehouseRequest, warehouse_movement},
};
use super::*;
use commerce_domain::{
    ComponentKind,
    catalogue_authoring::{BomLineDraft, CatalogueCommand, Fill},
    inventory::WarehouseChange,
};
async fn define(pool: &PgPool, key: &str, command: CatalogueCommand) {
    record_definition(
        pool,
        &SessionToken::from_bytes([0xbd; 32]),
        &Time(AtomicU64::new(1100)),
        DefinitionRequest {
            effect_key: key,
            command: &command,
            source_ref: "invented isolated basket fixture; never supplier evidence",
            reason: "isolated source",
            confirmed: true,
        },
    )
    .await
    .expect("synthetic unpublished definition");
}
pub async fn variant(pool: &PgPool, sku: &str, filled: bool, components: &[(&str, u32)]) {
    define(
        pool,
        sku,
        CatalogueCommand::Variant {
            sku: sku.into(),
            family_code: "ROOM-A".into(),
            size_code: "isolated".into(),
            textile_code: "isolated".into(),
            fill: if filled {
                Fill::WithFoam
            } else {
                Fill::CoversOnly
            },
            lines: components
                .iter()
                .map(|(code, quantity)| BomLineDraft {
                    component_code: (*code).into(),
                    quantity: *quantity,
                })
                .collect(),
        },
    )
    .await;
}
pub async fn receive(pool: &PgPool, component: &str, quantity: u32) {
    if quantity == 0 {
        return;
    }
    warehouse_movement(
        pool,
        WarehouseRequest {
            effect_key: component,
            component_code: component,
            change: WarehouseChange::Receive(quantity),
            actor: "fixture-warehouse",
            reason: "invented isolated receipts",
        },
    )
    .await
    .expect("receive");
}
pub async fn seed(pool: &PgPool, covers: u32, foam: u32) {
    pool.execute("INSERT INTO staff_accounts VALUES('basket-cataloguer',true);INSERT INTO staff_capabilities VALUES('basket-cataloguer','catalogue.manage')").await.expect("test authority");
    sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,'basket-cataloguer',1000,4600,1100)").bind(SessionToken::from_bytes([0xbd;32]).digest().as_slice()).execute(pool).await.expect("test session");
    define(
        pool,
        "family",
        CatalogueCommand::Family {
            code: "ROOM-A".into(),
        },
    )
    .await;
    for (key, code, kind) in [
        ("cover", "SEAT-COVER", ComponentKind::Cover),
        ("foam", "SEAT-FOAM", ComponentKind::Foam),
        ("other", "OTHER-COVER", ComponentKind::Cover),
    ] {
        define(
            pool,
            key,
            CatalogueCommand::Component {
                code: code.into(),
                component_kind: kind,
            },
        )
        .await;
    }
    variant(pool, "SET-COVERS", false, &[("SEAT-COVER", 2)]).await;
    variant(
        pool,
        "SET-FILLED",
        true,
        &[("SEAT-COVER", 2), ("SEAT-FOAM", 2)],
    )
    .await;
    receive(pool, "SEAT-COVER", covers).await;
    receive(pool, "SEAT-FOAM", foam).await;
}
pub async fn balances(pool: &PgPool) -> Vec<(String, i64, i64, i64)> {
    sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances ORDER BY component_code").fetch_all(pool).await.expect("physical balances")
}
pub async fn counts(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM basket_hold_groups),(SELECT count(*) FROM basket_hold_members),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements WHERE action='reserve')").fetch_one(pool).await.expect("effect counts")
}
pub async fn conserved(pool: &PgPool) {
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM stock_balances b WHERE (b.on_hand,b.quarantine,b.reserved) IS DISTINCT FROM (SELECT ROW(sum(on_hand_delta)::bigint,sum(quarantine_delta)::bigint,sum(reserved_delta)::bigint) FROM stock_movements m WHERE m.component_code=b.component_code)").fetch_one(pool).await.expect("independent conservation");
    assert_eq!(count, 0);
}
