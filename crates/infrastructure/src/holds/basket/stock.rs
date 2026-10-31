use super::*;
#[derive(sqlx::FromRow)]
struct Balance {
    component_code: String,
    on_hand: i64,
    quarantine: i64,
    reserved: i64,
}
pub(super) async fn claim(
    tx: &mut Transaction<'_, Postgres>,
    plans: &[Plan],
) -> Result<(), HoldWriteError> {
    let demand = demand(plans)?;
    let codes: Vec<_> = demand.keys().copied().collect();
    // Both missing-balance insertion and UPDATE claims use the global physical-component order.
    sqlx::query("INSERT INTO stock_balances(component_code) SELECT code FROM unnest($1::text[]) AS selected(code) ORDER BY code ON CONFLICT DO NOTHING")
        .bind(&codes).execute(&mut **tx).await?;
    let rows:Vec<Balance>=sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code=ANY($1) ORDER BY component_code FOR UPDATE").bind(&codes).fetch_all(&mut **tx).await?;
    validate(rows, &demand)
}
fn demand(plans: &[Plan]) -> Result<BTreeMap<&str, i64>, HoldWriteError> {
    let mut demand = BTreeMap::<&str, i64>::new();
    for plan in plans {
        for component in &plan.bom.components {
            let needed = i64::from(component.quantity)
                .checked_mul(i64::from(plan.line.units))
                .ok_or(HoldWriteError::InvalidRequest)?;
            let total = demand.entry(component.code.as_str()).or_default();
            *total = total
                .checked_add(needed)
                .ok_or(HoldWriteError::InvalidRequest)?;
        }
    }
    Ok(demand)
}
pub(super) async fn check_reserved(
    tx: &mut Transaction<'_, Postgres>,
    plans: &[Plan],
) -> Result<(), HoldWriteError> {
    let demand = demand(plans)?;
    let codes: Vec<_> = demand.keys().copied().collect();
    let rows:Vec<Balance>=sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code=ANY($1) ORDER BY component_code FOR SHARE").bind(&codes).fetch_all(&mut **tx).await?;
    if rows.len() != demand.len() {
        return Err(HoldWriteError::InvalidRequest);
    }
    for row in rows {
        let needed = demand
            .get(row.component_code.as_str())
            .ok_or(HoldWriteError::InvalidRequest)?;
        let balance = StockBalance::new(row.on_hand, row.quarantine, row.reserved)?;
        if balance.reserved() < *needed {
            return Err(commerce_domain::inventory::StockError::InvalidBalance.into());
        }
    }
    Ok(())
}
fn validate(rows: Vec<Balance>, demand: &BTreeMap<&str, i64>) -> Result<(), HoldWriteError> {
    if rows.len() != demand.len() {
        return Err(HoldWriteError::InvalidRequest);
    }
    for row in rows {
        let needed = demand
            .get(row.component_code.as_str())
            .ok_or(HoldWriteError::InvalidRequest)?;
        StockBalance::new(row.on_hand, row.quarantine, row.reserved)?.reserve(*needed)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn row(code: &str, on_hand: i64, quarantine: i64, reserved: i64) -> Balance {
        Balance {
            component_code: code.into(),
            on_hand,
            quarantine,
            reserved,
        }
    }
    #[test]
    fn incomplete_or_damaged_physical_claims_never_grant_a_basket() {
        let demand = BTreeMap::from([("COVER", 4)]);
        assert!(validate(vec![row("COVER", 6, 1, 1)], &demand).is_ok());
        for rows in [
            vec![],
            vec![row("OTHER", 6, 0, 0)],
            vec![row("COVER", 6, 2, 1)],
            vec![row("COVER", -1, 0, 0)],
            vec![row("COVER", 6, 0, 0), row("COVER", 6, 0, 0)],
        ] {
            assert!(validate(rows, &demand).is_err());
        }
    }
}
