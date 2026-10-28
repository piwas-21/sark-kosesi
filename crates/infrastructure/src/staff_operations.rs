//! Scoped staff operations; actor identity always comes from the session guard.
use super::{
    staff_access::{SessionToken, StaffError, lock_staff},
    stock::{MovementReceipt, WarehouseRequest, finish_movement, movement_in_transaction},
};
use commerce_application::{request_context::Clock, staff_access::Capability};
use commerce_domain::{BillOfMaterials, inventory::WarehouseChange};
use sqlx::PgPool;
pub struct StaffWarehouseRequest<'a> {
    pub effect_key: &'a str,
    pub component_code: &'a str,
    pub change: WarehouseChange,
    pub reason: &'a str,
    pub confirmed: bool,
}
pub async fn warehouse_change(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: StaffWarehouseRequest<'_>,
) -> Result<MovementReceipt, StaffError> {
    let mut access = lock_staff(pool, token, clock, Capability::WarehouseManage).await?;
    if !request.confirmed {
        return Err(StaffError::InvalidRequest);
    }
    let result = movement_in_transaction(
        &mut access.tx,
        WarehouseRequest {
            effect_key: request.effect_key,
            component_code: request.component_code,
            change: request.change,
            actor: &access.actor,
            reason: request.reason,
        },
    )
    .await;
    if result.is_ok() {
        // Inventory/ledger locks may also wait. Expiry during that wait must roll
        // back the pending effects before any receipt or commit becomes visible.
        if let Err(error) = access.check_time(clock) {
            access.tx.rollback().await?;
            return Err(error);
        }
    }
    finish_movement(access.tx, result)
        .await
        .map_err(StaffError::from)
}
pub async fn catalogue_bom(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    sku: &str,
    revision: i32,
) -> Result<Option<BillOfMaterials>, StaffError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if sku.is_empty() || sku.len() > 65 || revision < 1 {
        return Err(StaffError::InvalidRequest);
    }
    let bom = super::bom::load(&mut *access.tx, sku, revision).await?;
    access.check_time(clock)?;
    access.tx.rollback().await?;
    Ok(bom)
}
