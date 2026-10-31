use super::*;

#[derive(Clone, Debug)]
pub struct PricedItem {
    pub sku: String,
    pub publication_revision: i32,
    pub quantity: u32,
    pub unit_gross: Money,
    pub vat_basis_points: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AmountBreakdown {
    pub gross: Money,
    pub net: Money,
    pub vat: Money,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QuoteLine {
    pub sku: String,
    pub publication_revision: i32,
    pub quantity: u32,
    pub unit_gross: Money,
    pub vat_basis_points: u16,
    pub amounts: AmountBreakdown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QuoteTotals {
    pub lines: Vec<QuoteLine>,
    pub shipping: AmountBreakdown,
    pub shipping_vat_basis_points: u16,
    pub total: AmountBreakdown,
}
fn money(value: i64) -> Result<Money, QuoteError> {
    Money::eur(value)
        .ok()
        .filter(|m| bounded_money(*m))
        .ok_or(QuoteError::AmountTooLarge)
}
fn wide_money(value: i128) -> Result<Money, QuoteError> {
    money(i64::try_from(value).map_err(|_| QuoteError::AmountTooLarge)?)
}
fn add(left: Money, right: Money) -> Result<Money, QuoteError> {
    money(
        left.checked_add(right)
            .map_err(|_| QuoteError::AmountTooLarge)?
            .minor_units(),
    )
}
/// VAT-inclusive gross. Round VAT half up once per extended SKU line and shipping.
/// The total sums those allocations; it never recomputes VAT over mixed rates.
pub(crate) fn split(gross: Money, rate: u16) -> Result<AmountBreakdown, QuoteError> {
    if !valid_rate(rate) {
        return Err(QuoteError::InvalidPolicy);
    }
    let gross = money(gross.minor_units())?;
    let denominator = 10_000 + i128::from(rate);
    let vat = (i128::from(gross.minor_units()) * i128::from(rate) + denominator / 2) / denominator;
    let vat = wide_money(vat)?;
    Ok(AmountBreakdown {
        gross,
        net: money(gross.minor_units() - vat.minor_units())?,
        vat,
    })
}
pub fn calculate(
    items: &[PricedItem],
    shipping_gross: Money,
    shipping_rate: u16,
) -> Result<QuoteTotals, QuoteError> {
    let mut seen = BTreeSet::new();
    if items.is_empty()
        || items.len() > MAX_LINES
        || !items.iter().all(|p| {
            merchant_code(&p.sku)
                && p.publication_revision > 0
                && (1..=MAX_QUANTITY).contains(&p.quantity)
                && seen.insert(&p.sku)
        })
    {
        return Err(QuoteError::InvalidRequest);
    }
    let shipping = split(shipping_gross, shipping_rate)?;
    let mut total = shipping.clone();
    let mut lines = Vec::with_capacity(items.len());
    let mut sorted: Vec<_> = items.iter().collect();
    sorted.sort_by(|a, b| a.sku.cmp(&b.sku));
    for item in sorted {
        let gross = item
            .unit_gross
            .checked_multiply(item.quantity)
            .map_err(|_| QuoteError::AmountTooLarge)?;
        let amounts = split(gross, item.vat_basis_points)?;
        total = AmountBreakdown {
            gross: add(total.gross, amounts.gross)?,
            net: add(total.net, amounts.net)?,
            vat: add(total.vat, amounts.vat)?,
        };
        lines.push(QuoteLine {
            sku: item.sku.clone(),
            publication_revision: item.publication_revision,
            quantity: item.quantity,
            unit_gross: item.unit_gross,
            vat_basis_points: item.vat_basis_points,
            amounts,
        });
    }
    Ok(QuoteTotals {
        lines,
        shipping,
        shipping_vat_basis_points: shipping_rate,
        total,
    })
}

#[cfg(test)]
#[path = "../../tests/support/quote_money.rs"]
mod tests;
