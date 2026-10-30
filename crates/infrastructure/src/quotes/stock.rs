use super::*;
use commerce_domain::{inventory::StockBalance, quotes::BasketLine};
use sqlx::{Postgres, Transaction};
use std::collections::BTreeMap;

#[derive(sqlx::FromRow)]
struct Balance {
    component_code: String,
    on_hand: i64,
    quarantine: i64,
    reserved: i64,
}
pub(super) async fn check(
    tx: &mut Transaction<'_, Postgres>,
    products: &[crate::published_catalogue::EligibleProduct],
    lines: &[BasketLine],
) -> Result<(), QuoteFailure> {
    let mut demand = BTreeMap::<&str, i64>::new();
    for eligible in products {
        let product = &eligible.product;
        let quantity = lines
            .iter()
            .find(|l| l.sku == product.sku)
            .ok_or(QuoteFailure::InvalidStoredData)?
            .quantity;
        for component in &product.components {
            let units = i64::from(component.quantity)
                .checked_mul(i64::from(quantity))
                .ok_or(QuoteFailure::InvalidStoredData)?;
            let current = demand.entry(&component.code).or_default();
            *current = current
                .checked_add(units)
                .ok_or(QuoteFailure::InvalidStoredData)?;
        }
    }
    let codes: Vec<_> = demand.keys().copied().collect();
    // Same global component order as reservations; no fictitious per-SKU pools.
    let rows = sqlx::query_as::<_,Balance>("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code=ANY($1) ORDER BY component_code FOR SHARE")
        .bind(&codes).fetch_all(&mut **tx).await?;
    if rows.len() != codes.len() {
        return Err(QuoteError::InsufficientStock.into());
    }
    for row in rows {
        let needed = demand
            .get(row.component_code.as_str())
            .ok_or(QuoteFailure::InvalidStoredData)?;
        let balance = StockBalance::new(row.on_hand, row.quarantine, row.reserved)
            .map_err(|_| QuoteFailure::InvalidStoredData)?;
        if balance.available() < *needed {
            return Err(QuoteError::InsufficientStock.into());
        }
    }
    Ok(())
}
