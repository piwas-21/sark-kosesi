//! Read claims over an existing complete reservation; never reserve or reactivate stock.
use super::*;
pub(crate) struct ExistingBasketLine<'a> {
    pub sku: &'a str,
    pub revision: i32,
    pub units: u16,
}
pub(crate) struct ExistingBasketBinding<'a> {
    pub id: &'a str,
    pub owner_id: &'a str,
    pub created_at: i64,
    pub expires_at: i64,
    pub lines: &'a [ExistingBasketLine<'a>],
}
pub(crate) struct ActiveBasket {
    window: HoldWindow,
    checked_at: i64,
}
impl ActiveBasket {
    pub(crate) fn check_at(&mut self, at: i64) -> Result<(), HoldWriteError> {
        if at < self.checked_at {
            return Err(HoldWriteError::ClockUnavailable);
        }
        self.window
            .transition(HoldState::Active, HoldAction::Consume, at)?;
        self.checked_at = at;
        Ok(())
    }
}
#[derive(sqlx::FromRow)]
struct Group {
    owner_id: String,
    created_at: i64,
    expires_at: i64,
    members_count: i32,
    sealed: bool,
}
#[derive(sqlx::FromRow)]
struct Member {
    id: String,
    sku: String,
    revision: i32,
    units: i32,
    owner_id: String,
    created_at: i64,
    expires_at: i64,
    state: String,
    matches: bool,
}
/// Caller retains current catalogue/market claims first. Member SHARE claims precede
/// balance SHARE claims, matching expiry's hold-before-component order without upgrades.
pub(crate) async fn claim_active(
    tx: &mut Transaction<'_, Postgres>,
    clock: &dyn Clock,
    binding: ExistingBasketBinding<'_>,
) -> Result<ActiveBasket, HoldWriteError> {
    let mut seen = BTreeSet::new();
    if !identifier(binding.id)
        || !identifier(binding.owner_id)
        || binding.lines.is_empty()
        || binding.lines.len() > 20
        || !binding.lines.iter().all(|line| {
            merchant_code(line.sku)
                && line.revision > 0
                && (1..=1000).contains(&line.units)
                && seen.insert(line.sku)
        })
    {
        return Err(HoldWriteError::InvalidRequest);
    }
    let at = now(clock)?;
    let mut proof = ActiveBasket {
        window: HoldWindow::new(binding.created_at, binding.expires_at)?,
        checked_at: at,
    };
    proof.check_at(at)?;
    let group:Group=sqlx::query_as("SELECT left(owner_id,33) AS owner_id,created_at,expires_at,members_count,sealed FROM basket_hold_groups WHERE id=$1 FOR SHARE").bind(binding.id).fetch_optional(&mut **tx).await?.ok_or(HoldWriteError::NotFound)?;
    if !group.sealed
        || group.owner_id != binding.owner_id
        || group.created_at != binding.created_at
        || group.expires_at != binding.expires_at
        || usize::try_from(group.members_count).ok() != Some(binding.lines.len())
    {
        return Err(HoldWriteError::InvalidRequest);
    }
    let mut members:Vec<Member>=sqlx::query_as("SELECT left(h.id,33) AS id,left(h.sku,66) AS sku,h.revision,h.units,left(h.owner_id,33) AS owner_id,h.created_at,h.expires_at,left(h.state,10) AS state,(m.sku,m.revision,m.units,m.group_id)=(h.sku,h.revision,h.units,h.basket_group_id) AS matches FROM basket_hold_members m JOIN stock_holds h ON h.id=m.hold_id WHERE m.group_id=$1 ORDER BY h.id LIMIT 21 FOR SHARE OF h").bind(binding.id).fetch_all(&mut **tx).await?;
    if members.len() != binding.lines.len() {
        return Err(HoldWriteError::InvalidRequest);
    }
    members.sort_by(|a, b| a.sku.cmp(&b.sku));
    let mut expected: Vec<_> = binding.lines.iter().collect();
    expected.sort_by(|a, b| a.sku.cmp(b.sku));
    let mut selected = Vec::with_capacity(members.len());
    for (member, line) in members.into_iter().zip(expected) {
        if !member.matches
            || !identifier(&member.id)
            || member.sku != line.sku
            || member.revision != line.revision
            || member.units != i32::from(line.units)
            || member.owner_id != binding.owner_id
            || member.created_at != binding.created_at
            || member.expires_at != binding.expires_at
            || !["active", "released", "consumed", "expired"].contains(&member.state.as_str())
        {
            return Err(HoldWriteError::InvalidRequest);
        }
        if member.state != "active" {
            return Err(commerce_domain::holds::HoldError::InvalidTransition.into());
        }
        selected.push(BasketHoldLine {
            id: member.id,
            sku: member.sku,
            revision: member.revision,
            units: line.units,
        });
    }
    let plans = prepare(tx, &selected).await?;
    for plan in &plans {
        let codes: Vec<_> = plan
            .bom
            .components
            .iter()
            .map(|c| c.code.as_str())
            .collect();
        // Loaded BOM quantities <=10000 and validated units <=1000 fit i64 exactly.
        let quantities: Vec<_> = plan
            .bom
            .components
            .iter()
            .map(|c| i64::from(c.quantity) * i64::from(plan.line.units))
            .collect();
        let matches: bool = sqlx::query_scalar("SELECT NOT EXISTS((SELECT component_code,quantity FROM stock_hold_lines WHERE hold_id=$1) EXCEPT (SELECT * FROM unnest($2::text[],$3::bigint[]))) AND NOT EXISTS((SELECT * FROM unnest($2::text[],$3::bigint[])) EXCEPT (SELECT component_code,quantity FROM stock_hold_lines WHERE hold_id=$1))")
            .bind(&plan.line.id).bind(codes).bind(quantities).fetch_one(&mut **tx).await?;
        if !matches {
            return Err(HoldWriteError::InvalidRequest);
        }
    }
    stock::check_reserved(tx, &plans).await?;
    proof.check_at(now(clock)?)?;
    Ok(proof)
}

#[cfg(test)]
#[path = "../../../tests/support/active_basket.rs"]
mod tests;
