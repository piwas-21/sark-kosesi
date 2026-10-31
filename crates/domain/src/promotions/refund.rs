use super::*;

fn vat_prefix(snapshot: &AmountBreakdown, gross: Money) -> Result<Money, PromotionError> {
    let denominator = i128::from(snapshot.gross.minor_units());
    let numerator = i128::from(snapshot.vat.minor_units())
        .checked_mul(i128::from(gross.minor_units()))
        .and_then(|n| n.checked_add(denominator / 2))
        .ok_or(PromotionError::AmountTooLarge)?;
    wide_money(numerator / denominator)
}
/// Allocate VAT by the difference between cumulative half-up snapshot shares.
/// Full repayment returns the exact captured VAT/net, independent of later prices.
/// The caller must serialize and persist previous gross; this does not issue refunds.
pub fn allocate_refund(
    snapshot: &AmountBreakdown,
    already_refunded_gross: Money,
    requested_gross: Money,
) -> Result<AmountBreakdown, PromotionError> {
    if snapshot.gross.minor_units() == 0
        || requested_gross.minor_units() == 0
        || snapshot.gross.minor_units() > MAX_MINOR_UNITS
        || snapshot
            .net
            .checked_add(snapshot.vat)
            .map_err(|_| PromotionError::InvalidRefund)?
            != snapshot.gross
    {
        return Err(PromotionError::InvalidRefund);
    }
    let cumulative = add(already_refunded_gross, requested_gross)?;
    if cumulative.minor_units() > snapshot.gross.minor_units() {
        return Err(PromotionError::InvalidRefund);
    }
    let vat = subtract(
        vat_prefix(snapshot, cumulative)?,
        vat_prefix(snapshot, already_refunded_gross)?,
    )?;
    Ok(AmountBreakdown {
        gross: requested_gross,
        net: subtract(requested_gross, vat)?,
        vat,
    })
}
