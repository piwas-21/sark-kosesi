//! Exact original reserve/terminal custody and immutable settlement movement proof.
use super::*;
#[derive(sqlx::FromRow)]
struct Movement {
    hold_id: String,
    component_code: String,
    action: String,
    effect_key: String,
    quantity: i64,
    actor: String,
    reason: String,
    on_hand_delta: i64,
    quarantine_delta: i64,
    reserved_delta: i64,
    on_hand_after: i64,
    quarantine_after: i64,
    reserved_after: i64,
    sequence: i64,
}
async fn movements(
    tx: &mut Transaction<'_, Postgres>,
    members: &[Member],
) -> Result<Vec<Movement>, SettlementError> {
    let holds: Vec<_> = members.iter().map(|m| m.hold_id.as_str()).collect();
    let rows: Vec<Movement> = sqlx::query_as("SELECT left(hold_id,33) AS hold_id,left(component_code,66) AS component_code,left(action,20) AS action,left(effect_key,129) AS effect_key,quantity,left(actor,129) AS actor,left(reason,501) AS reason,on_hand_delta,quarantine_delta,reserved_delta,on_hand_after,quarantine_after,reserved_after,sequence FROM stock_movements WHERE hold_id=ANY($1) ORDER BY sequence LIMIT 2561")
        .bind(holds).fetch_all(&mut **tx).await?;
    if rows.len() > MAX_PAIRS * 2 {
        return Err(SettlementError::InvalidStoredData);
    }
    Ok(rows)
}
fn exact(
    row: &Movement,
    hold: &str,
    quantity: &Quantity,
    action: &str,
) -> Result<(), SettlementError> {
    let reserve = action == "reserve";
    let consume = action == "consume-hold";
    if row.hold_id != hold
        || row.component_code != quantity.component_code
        || row.action != action
        || row.effect_key != format!("hold:{hold}:{action}:{}", quantity.component_code)
        || row.quantity != quantity.quantity
        || row.reason != "sealed BOM hold transition"
        || row.actor.trim().is_empty()
        || row.actor.chars().count() > 128
        || row.on_hand_delta != if consume { -quantity.quantity } else { 0 }
        || row.quarantine_delta != 0
        || row.reserved_delta
            != if reserve {
                quantity.quantity
            } else {
                -quantity.quantity
            }
        || row.sequence <= 0
    {
        return Err(SettlementError::InvalidStoredData);
    }
    Balance {
        on_hand: row.on_hand_after,
        quarantine: row.quarantine_after,
        reserved: row.reserved_after,
    }
    .stock()?;
    if reserve && row.reserved_after < quantity.quantity {
        return Err(SettlementError::InvalidStoredData);
    }
    Ok(())
}
pub(super) async fn source(
    tx: &mut Transaction<'_, Postgres>,
    members: &[Member],
    states: &[String],
    owner: &str,
) -> Result<(), SettlementError> {
    let rows = movements(tx, members).await?;
    let mut expected = 0;
    for (member, state) in members.iter().zip(states) {
        for quantity in &member.components {
            let reserve = rows
                .iter()
                .find(|r| {
                    r.hold_id == member.hold_id
                        && r.component_code == quantity.component_code
                        && r.action == "reserve"
                })
                .ok_or(SettlementError::InvalidStoredData)?;
            exact(reserve, &member.hold_id, quantity, "reserve")?;
            if reserve.actor != owner {
                return Err(SettlementError::InvalidStoredData);
            }
            expected += 1;
            let terminal = match state.as_str() {
                "active" => None,
                "released" => Some("release-hold"),
                "expired" => Some("expire-hold"),
                "consumed" => Some("consume-hold"),
                _ => return Err(SettlementError::InvalidStoredData),
            };
            if let Some(action) = terminal {
                let row = rows
                    .iter()
                    .find(|r| {
                        r.hold_id == member.hold_id
                            && r.component_code == quantity.component_code
                            && r.action == action
                    })
                    .ok_or(SettlementError::InvalidStoredData)?;
                exact(row, &member.hold_id, quantity, action)?;
                if row.sequence <= reserve.sequence {
                    return Err(SettlementError::InvalidStoredData);
                }
                expected += 1;
            }
        }
    }
    if rows.len() != expected {
        return Err(SettlementError::InvalidStoredData);
    }
    Ok(())
}
pub(super) async fn settled(
    tx: &mut Transaction<'_, Postgres>,
    settlement: &Settlement,
) -> Result<(), SettlementError> {
    let rows = movements(tx, &settlement.footprint.members).await?;
    let mut balances: std::collections::BTreeMap<_, _> = settlement
        .footprint
        .components
        .iter()
        .map(|component| (component.component_code.as_str(), component.before.clone()))
        .collect();
    let mut previous = 0;
    for member in &settlement.footprint.members {
        for quantity in &member.components {
            let row = rows
                .iter()
                .find(|r| {
                    r.hold_id == member.hold_id
                        && r.component_code == quantity.component_code
                        && r.action == "consume-hold"
                })
                .ok_or(SettlementError::InvalidStoredData)?;
            exact(row, &member.hold_id, quantity, "consume-hold")?;
            let balance = balances
                .get_mut(quantity.component_code.as_str())
                .ok_or(SettlementError::InvalidStoredData)?;
            *balance = balance.consume(quantity.quantity)?;
            if row.actor != ACTOR
                || row.sequence <= previous
                || row.on_hand_after != balance.on_hand
                || row.quarantine_after != balance.quarantine
                || row.reserved_after != balance.reserved
            {
                return Err(SettlementError::InvalidStoredData);
            }
            previous = row.sequence;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/support/order_stock_effects_unit.rs"]
mod decoder_tests;
