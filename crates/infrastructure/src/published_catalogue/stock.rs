use super::*;
#[derive(sqlx::FromRow)]
struct Balance {
    component_code: String,
    quantity: i32,
    on_hand: Option<i64>,
    quarantine: Option<i64>,
    reserved: Option<i64>,
}
impl Balance {
    fn available(&self) -> Result<i64, PublishedError> {
        match (self.on_hand, self.quarantine, self.reserved) {
            (None, None, None) => Ok(0),
            (Some(on_hand), Some(quarantine), Some(reserved)) => {
                StockBalance::new(on_hand, quarantine, reserved)
                    .map(StockBalance::available)
                    .map_err(|_| PublishedError::InvalidStoredData)
            }
            _ => Err(PublishedError::InvalidStoredData),
        }
    }
}
pub(super) async fn available(
    tx: &mut Transaction<'_, Postgres>,
    sku: &str,
    revision: i32,
    lines: &[BomLineDraft],
) -> Result<bool, PublishedError> {
    // One statement sees one stock snapshot across every physical component.
    let rows=sqlx::query_as::<_,Balance>("SELECT l.component_code,l.quantity,b.on_hand,b.quarantine,b.reserved FROM catalogue_bom_lines l LEFT JOIN stock_balances b USING(component_code) WHERE l.sku=$1 AND l.revision=$2 ORDER BY l.component_code")
        .bind(sku).bind(revision).fetch_all(&mut **tx).await?;
    available_rows(rows, lines)
}
fn available_rows(rows: Vec<Balance>, lines: &[BomLineDraft]) -> Result<bool, PublishedError> {
    if rows.len() != lines.len() || rows.is_empty() {
        return Err(PublishedError::InvalidStoredData);
    }
    let mut available = i64::MAX;
    for row in rows {
        let expected = lines
            .iter()
            .find(|line| line.component_code == row.component_code)
            .ok_or(PublishedError::InvalidStoredData)?;
        if row.quantity <= 0 || u32::try_from(row.quantity).ok() != Some(expected.quantity) {
            return Err(PublishedError::InvalidStoredData);
        }
        available = available.min(row.available()? / i64::from(row.quantity));
    }
    Ok(available > 0)
}

#[cfg(test)]
#[path = "../../tests/support/published_stock.rs"]
mod tests;
