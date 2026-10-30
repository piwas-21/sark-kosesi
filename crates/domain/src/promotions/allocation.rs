use super::*;

fn validate_quote(quote: &QuoteTotals) -> Result<(), PromotionError> {
    if quote.lines.is_empty() || quote.lines.len() > MAX_LINES {
        return Err(PromotionError::InvalidQuote);
    }
    let items: Vec<_> = quote
        .lines
        .iter()
        .map(|l| PricedItem {
            sku: l.sku.clone(),
            publication_revision: l.publication_revision,
            quantity: l.quantity,
            unit_gross: l.unit_gross,
            vat_basis_points: l.vat_basis_points,
        })
        .collect();
    let recalculated = calculate(
        &items,
        quote.shipping.gross,
        quote.shipping_vat_basis_points,
    )
    .map_err(|_| PromotionError::InvalidQuote)?;
    if recalculated != *quote {
        return Err(PromotionError::InvalidQuote);
    }
    Ok(())
}
fn allocations(
    quote: &QuoteTotals,
    promotion: &Promotion,
) -> Result<(Money, BTreeMap<String, Money>), PromotionError> {
    let mut eligible = Vec::new();
    let mut subtotal = money(0)?;
    for line in &quote.lines {
        if let Some(rule) = promotion.products.iter().find(|p| p.sku == line.sku) {
            if rule.publication_revision != line.publication_revision {
                return Err(PromotionError::Ineligible);
            }
            subtotal = add(subtotal, line.amounts.gross)?;
            eligible.push(line);
        }
    }
    if subtotal.minor_units() == 0
        || subtotal.minor_units() < promotion.minimum_eligible_gross_minor
    {
        return Err(PromotionError::Ineligible);
    }
    let requested = match promotion.discount {
        DiscountKind::Fixed { minor_units } => money(minor_units)?,
        DiscountKind::Percentage { basis_points } => {
            let numerator = i128::from(subtotal.minor_units())
                .checked_mul(i128::from(basis_points))
                .and_then(|n| n.checked_add(5000))
                .ok_or(PromotionError::AmountTooLarge)?;
            wide_money(numerator / 10000)?
        }
    };
    let discount = money(
        requested
            .minor_units()
            .min(promotion.maximum_discount_minor)
            .min(subtotal.minor_units()),
    )?;
    if discount.minor_units() == 0 {
        return Err(PromotionError::Ineligible);
    }
    let denominator = i128::from(subtotal.minor_units());
    let mut rows = Vec::with_capacity(eligible.len());
    let mut floor_total = money(0)?;
    for line in eligible {
        let numerator = i128::from(discount.minor_units())
            .checked_mul(i128::from(line.amounts.gross.minor_units()))
            .ok_or(PromotionError::AmountTooLarge)?;
        let floor = wide_money(numerator / denominator)?;
        floor_total = add(floor_total, floor)?;
        rows.push((line.sku.clone(), floor, numerator % denominator));
    }
    rows.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    let remainder = usize::try_from(subtract(discount, floor_total)?.minor_units())
        .map_err(|_| PromotionError::AmountTooLarge)?;
    let mut result = BTreeMap::new();
    for (index, (sku, floor, _)) in rows.into_iter().enumerate() {
        let allocated = if index < remainder {
            add(floor, money(1)?)?
        } else {
            floor
        };
        result.insert(sku, allocated);
    }
    Ok((discount, result))
}
/// Largest remainder apportions the single eligible-basket discount, ties by SKU.
/// VAT is then split on each discounted extended line; shipping is unchanged.
pub fn apply(
    quote: &QuoteTotals,
    promotion: &Promotion,
    country: &str,
    now: i64,
) -> Result<PromotionQuote, PromotionError> {
    promotion.permits(country, now)?;
    validate_quote(quote)?;
    let (discount, allocated) = allocations(quote, promotion)?;
    let mut total = quote.shipping.clone();
    let mut lines = Vec::with_capacity(quote.lines.len());
    for original in &quote.lines {
        let discount = allocated.get(&original.sku).copied().unwrap_or(money(0)?);
        let gross = subtract(original.amounts.gross, discount)?;
        let amounts = crate::quotes::split(gross, original.vat_basis_points)
            .map_err(|_| PromotionError::InvalidQuote)?;
        total = AmountBreakdown {
            gross: add(total.gross, amounts.gross)?,
            net: add(total.net, amounts.net)?,
            vat: add(total.vat, amounts.vat)?,
        };
        lines.push(DiscountedLine {
            original: original.clone(),
            discount,
            amounts,
        });
    }
    Ok(PromotionQuote {
        allocation_version: ALLOCATION_VERSION,
        promotion_id: promotion.id.clone(),
        promotion_revision: promotion.revision,
        discount,
        lines,
        shipping: quote.shipping.clone(),
        shipping_vat_basis_points: quote.shipping_vat_basis_points,
        total,
    })
}
