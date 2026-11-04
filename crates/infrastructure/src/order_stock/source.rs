//! Bounded original manifest validation and globally ordered physical claims.
use super::*;
#[derive(sqlx::FromRow)]
struct Group {
    owner_id: String,
    created_at: i64,
    expires_at: i64,
    members_count: i32,
    sealed: bool,
    actual_holds: i64,
    actual_members: i64,
}
#[derive(sqlx::FromRow)]
struct Hold {
    id: String,
    owner_id: String,
    sku: String,
    revision: i32,
    units: i32,
    created_at: i64,
    expires_at: i64,
    state: String,
    matches: bool,
}
#[derive(sqlx::FromRow)]
struct Line {
    component_code: String,
    quantity: i64,
    expected: Option<i64>,
    kind: Option<String>,
}
pub(crate) struct Members {
    pub members: Vec<Member>,
    pub active: bool,
    pub active_members: Vec<Member>,
    pub states: Vec<String>,
}
pub(crate) async fn attempt(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<String, SettlementError> {
    let id: String =
        sqlx::query_scalar("SELECT left(id,33) FROM payment_attempts WHERE order_id=$1")
            .bind(order)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(SettlementError::NotFound)?;
    if !identifier(&id) {
        return Err(SettlementError::InvalidStoredData);
    }
    Ok(id)
}
pub(crate) async fn claim_attempt(
    tx: &mut Transaction<'_, Postgres>,
    attempt: &str,
) -> Result<(), SettlementError> {
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(attempt)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
pub(crate) async fn members(
    tx: &mut Transaction<'_, Postgres>,
    order: &OrderSnapshot,
    claim: bool,
) -> Result<Members, SettlementError> {
    let group: Group = sqlx::query_as("SELECT left(owner_id,33) AS owner_id,created_at,expires_at,members_count,sealed,(SELECT count(*) FROM (SELECT 1 FROM stock_holds WHERE basket_group_id=g.id LIMIT 21) bounded) AS actual_holds,(SELECT count(*) FROM (SELECT 1 FROM basket_hold_members WHERE group_id=g.id LIMIT 21) bounded) AS actual_members FROM basket_hold_groups g WHERE g.id=$1 FOR SHARE")
        .bind(&order.identity.stock_group_id).fetch_optional(&mut **tx).await?.ok_or(SettlementError::InvalidStoredData)?;
    if !group.sealed
        || group.owner_id != order.identity.id
        || group.created_at != order.identity.created_at
        || group.expires_at != order.identity.hold_expires_at
        || !(1..=20).contains(&group.members_count)
        || i64::from(group.members_count) != group.actual_holds
        || group.actual_members != group.actual_holds
        || usize::try_from(group.members_count).ok() != Some(order.quote.lines.len())
    {
        return Err(SettlementError::InvalidStoredData);
    }
    let sql = if claim {
        "SELECT left(h.id,33) AS id,left(h.owner_id,33) AS owner_id,left(h.sku,66) AS sku,h.revision,h.units,h.created_at,h.expires_at,left(h.state,10) AS state,(m.sku,m.revision,m.units,m.group_id)=(h.sku,h.revision,h.units,h.basket_group_id) AS matches FROM basket_hold_members m JOIN stock_holds h ON h.id=m.hold_id WHERE m.group_id=$1 ORDER BY h.id LIMIT 21 FOR UPDATE OF h"
    } else {
        "SELECT left(h.id,33) AS id,left(h.owner_id,33) AS owner_id,left(h.sku,66) AS sku,h.revision,h.units,h.created_at,h.expires_at,left(h.state,10) AS state,(m.sku,m.revision,m.units,m.group_id)=(h.sku,h.revision,h.units,h.basket_group_id) AS matches FROM basket_hold_members m JOIN stock_holds h ON h.id=m.hold_id WHERE m.group_id=$1 ORDER BY h.id LIMIT 21 FOR SHARE OF h"
    };
    let holds: Vec<Hold> = sqlx::query_as(sql)
        .bind(&order.identity.stock_group_id)
        .fetch_all(&mut **tx)
        .await?;
    if holds.len() != order.quote.lines.len() {
        return Err(SettlementError::InvalidStoredData);
    }
    let mut members = Vec::with_capacity(holds.len());
    let mut states = Vec::with_capacity(holds.len());
    for hold in holds {
        let selected = order
            .quote
            .lines
            .iter()
            .find(|line| line.sku == hold.sku)
            .ok_or(SettlementError::InvalidStoredData)?;
        if !hold.matches
            || hold.owner_id != order.identity.id
            || hold.created_at != group.created_at
            || hold.expires_at != group.expires_at
            || hold.revision != selected.bom_revision
            || u32::try_from(hold.units).ok() != Some(selected.quantity)
            || !["active", "released", "expired", "consumed"].contains(&hold.state.as_str())
        {
            return Err(SettlementError::InvalidStoredData);
        }
        let lines: Vec<Line> = sqlx::query_as("SELECT left(l.component_code,66) AS component_code,l.quantity,(CASE WHEN b.quantity BETWEEN 1 AND 10000 THEN b.quantity::bigint*$4 END) AS expected,left(p.kind,8) AS kind FROM stock_hold_lines l LEFT JOIN catalogue_bom_lines b ON b.sku=$2 AND b.revision=$3 AND b.component_code=l.component_code LEFT JOIN physical_components p ON p.code=l.component_code WHERE l.hold_id=$1 ORDER BY l.component_code LIMIT 65")
            .bind(&hold.id).bind(&hold.sku).bind(hold.revision).bind(hold.units).fetch_all(&mut **tx).await?;
        let (sealed, count): (bool,i64) = sqlx::query_as("SELECT sealed,(SELECT count(*) FROM (SELECT 1 FROM catalogue_bom_lines WHERE sku=b.sku AND revision=b.revision LIMIT 65) bounded) FROM catalogue_boms b WHERE sku=$1 AND revision=$2")
            .bind(&hold.sku).bind(hold.revision).fetch_optional(&mut **tx).await?.ok_or(SettlementError::InvalidStoredData)?;
        if !sealed
            || !(1..=64).contains(&count)
            || usize::try_from(count).ok() != Some(lines.len())
            || lines.iter().any(|line| {
                Some(line.quantity) != line.expected
                    || !matches!(line.kind.as_deref(), Some("cover" | "foam"))
            })
        {
            return Err(SettlementError::InvalidStoredData);
        }
        members.push(Member {
            hold_id: hold.id,
            sku: hold.sku,
            revision: hold.revision,
            units: u16::try_from(hold.units).map_err(|_| SettlementError::InvalidStoredData)?,
            components: lines
                .into_iter()
                .map(|line| Quantity {
                    component_code: line.component_code,
                    quantity: line.quantity,
                })
                .collect(),
        });
        states.push(hold.state);
    }
    Footprint::demand(&members)?;
    effects::source(tx, &members, &states, &order.identity.id).await?;
    let active_members = members
        .iter()
        .zip(&states)
        .filter(|(_, state)| state.as_str() == "active")
        .map(|(member, _)| member.clone())
        .collect();
    Ok(Members {
        members,
        active_members,
        active: states.iter().all(|state| state == "active"),
        states,
    })
}
#[derive(sqlx::FromRow)]
struct Physical {
    component_code: String,
    on_hand: i64,
    quarantine: i64,
    reserved: i64,
}
pub(super) async fn balances(
    tx: &mut Transaction<'_, Postgres>,
    members: &[Member],
    active_members: &[Member],
) -> Result<Vec<Component>, SettlementError> {
    let demand = Footprint::demand(members)?;
    let active_demand = if active_members.is_empty() {
        std::collections::BTreeMap::new()
    } else {
        Footprint::demand(active_members)?
    };
    let active = active_members.len() == members.len();
    let codes: Vec<_> = demand.keys().map(String::as_str).collect();
    let rows: Vec<Physical> = sqlx::query_as("SELECT left(component_code,66) AS component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code=ANY($1) ORDER BY component_code LIMIT 1281 FOR UPDATE")
        .bind(&codes).fetch_all(&mut **tx).await?;
    if rows.len() != demand.len() {
        return Err(SettlementError::InvalidStoredData);
    }
    rows.into_iter()
        .map(|row| {
            let quantity = *demand
                .get(&row.component_code)
                .ok_or(SettlementError::InvalidStoredData)?;
            let before = Balance {
                on_hand: row.on_hand,
                quarantine: row.quarantine,
                reserved: row.reserved,
            };
            if before.stock()?.reserved()
                < active_demand.get(&row.component_code).copied().unwrap_or(0)
            {
                return Err(SettlementError::InvalidStoredData);
            }
            let after = if active {
                before.consume(quantity)?
            } else {
                before.clone()
            };
            Ok(Component {
                component_code: row.component_code,
                quantity,
                before,
                after,
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "../../tests/support/order_stock_source_unit.rs"]
mod source_tests;
