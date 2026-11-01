//! Fresh-only phases for a caller that owns financial/original-hold claims.
use super::*;
pub(crate) struct FreshPreparation {
    id: String,
    owner: String,
    created: i64,
    expires: i64,
    plans: Vec<Plan>,
}
impl FreshPreparation {
    pub(crate) fn members(
        &self,
    ) -> Result<Vec<commerce_domain::order_stock::Member>, HoldWriteError> {
        self.plans
            .iter()
            .map(|plan| {
                Ok(commerce_domain::order_stock::Member {
                    hold_id: plan.line.id.clone(),
                    sku: plan.line.sku.clone(),
                    revision: plan.line.revision,
                    units: plan.line.units,
                    components: plan
                        .bom
                        .components
                        .iter()
                        .map(|component| {
                            Ok(commerce_domain::order_stock::Quantity {
                                component_code: component.code.clone(),
                                quantity: i64::from(component.quantity)
                                    .checked_mul(i64::from(plan.line.units))
                                    .ok_or(HoldWriteError::InvalidRequest)?,
                            })
                        })
                        .collect::<Result<_, HoldWriteError>>()?,
                })
            })
            .collect()
    }
}
/// No replay/adoption path. Building child claims all precede component claims.
pub(crate) async fn prepare_fresh(
    tx: &mut Transaction<'_, Postgres>,
    request: BasketHoldRequest<'_>,
) -> Result<FreshPreparation, HoldWriteError> {
    let lines = selection(&request)?;
    sqlx::query("INSERT INTO basket_hold_groups(id,owner_id,created_at,expires_at,members_count) VALUES($1,$2,$3,$4,$5)")
        .bind(request.id).bind(request.owner_id).bind(request.created_at).bind(request.expires_at)
        .bind(i32::try_from(lines.len()).map_err(|_|HoldWriteError::InvalidRequest)?).execute(&mut **tx).await?;
    let plans = prepare(tx, &lines).await?;
    validate_plans(&plans)?;
    claim_children(tx, &request, &plans).await?;
    Ok(FreshPreparation {
        id: request.id.into(),
        owner: request.owner_id.into(),
        created: request.created_at,
        expires: request.expires_at,
        plans,
    })
}
/// The caller owns ALL union component UPDATE claims and has checked availability.
/// Existing basket reserve publication/order is deliberately reused unchanged.
pub(crate) async fn reserve_claimed(
    tx: &mut Transaction<'_, Postgres>,
    clock: &dyn Clock,
    fresh: &FreshPreparation,
) -> Result<(), HoldWriteError> {
    let lines: Vec<_> = fresh.plans.iter().map(|plan| plan.line.clone()).collect();
    let request = BasketHoldRequest {
        id: &fresh.id,
        owner_id: &fresh.owner,
        created_at: fresh.created,
        expires_at: fresh.expires,
        lines: &lines,
    };
    live(&request, fresh.created, now(clock)?)?;
    apply(tx, &request, &fresh.plans).await?;
    sqlx::query("UPDATE basket_hold_groups SET sealed=true WHERE id=$1")
        .bind(&fresh.id)
        .execute(&mut **tx)
        .await?;
    live(&request, fresh.created, now(clock)?)?;
    Ok(())
}

fn validate_plans(plans: &[Plan]) -> Result<(), HoldWriteError> {
    if plans.iter().any(|plan| {
        plan.bom.components.is_empty()
            || plan.bom.components.len() > commerce_domain::order_stock::MAX_LINES
    }) {
        return Err(HoldWriteError::InvalidRequest);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/support/late_stock_fresh_unit.rs"]
mod fresh_tests;
