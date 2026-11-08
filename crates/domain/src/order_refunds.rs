//! Pure allocations over an accepted order; no payment or refund authority.
use crate::{
    Money, checkout_orders::OrderSnapshot, checkout_quotes::CapturedAmount,
    promotions::allocate_refund, quotes::AmountBreakdown,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefundAllocationError {
    InvalidOrder,
    InvalidAllocation,
}

/// A complete caller-supplied balance for one SKU or shipping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefundAmounts {
    pub already_refunded_gross: Money,
    pub requested_gross: Money,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkuRefund {
    pub sku: String,
    pub amounts: RefundAmounts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderRefundAllocation {
    /// In the canonical order of the accepted quote, including zero-request lines.
    pub lines: Vec<(String, AmountBreakdown)>,
    pub shipping: AmountBreakdown,
    pub total: AmountBreakdown,
}

fn zero() -> Result<AmountBreakdown, RefundAllocationError> {
    let zero = Money::eur(0).map_err(|_| RefundAllocationError::InvalidAllocation)?;
    Ok(AmountBreakdown {
        gross: zero,
        net: zero,
        vat: zero,
    })
}

fn delta(
    captured: &AmountBreakdown,
    amounts: &RefundAmounts,
) -> Result<AmountBreakdown, RefundAllocationError> {
    if amounts.already_refunded_gross.minor_units() > captured.gross.minor_units() {
        return Err(RefundAllocationError::InvalidAllocation);
    }
    if amounts.requested_gross.minor_units() == 0 {
        return zero();
    }
    allocate_refund(
        captured,
        amounts.already_refunded_gross,
        amounts.requested_gross,
    )
    .map_err(|_| RefundAllocationError::InvalidAllocation)
}

fn sum(
    left: &AmountBreakdown,
    right: &AmountBreakdown,
) -> Result<AmountBreakdown, RefundAllocationError> {
    Ok(AmountBreakdown {
        gross: left
            .gross
            .checked_add(right.gross)
            .map_err(|_| RefundAllocationError::InvalidAllocation)?,
        net: left
            .net
            .checked_add(right.net)
            .map_err(|_| RefundAllocationError::InvalidAllocation)?,
        vat: left
            .vat
            .checked_add(right.vat)
            .map_err(|_| RefundAllocationError::InvalidAllocation)?,
    })
}

/// Compute explicit SKU/shipping deltas from accepted amounts and complete prior balances.
/// Callers must serialize, authenticate and persist the true prior balances. This operation
/// neither verifies payment capture nor creates a credit, provider refund or stock movement.
pub fn allocate_order_refund(
    order: &OrderSnapshot,
    lines: &[SkuRefund],
    shipping: &RefundAmounts,
) -> Result<OrderRefundAllocation, RefundAllocationError> {
    order
        .validate()
        .map_err(|_| RefundAllocationError::InvalidOrder)?;
    let quote = &order.quote;
    let breakdown = |amount: &CapturedAmount| {
        amount
            .breakdown()
            .map_err(|_| RefundAllocationError::InvalidOrder)
    };
    if lines.len() != quote.lines.len() {
        return Err(RefundAllocationError::InvalidAllocation);
    }
    let mut by_sku = BTreeMap::new();
    for line in lines {
        if by_sku.insert(line.sku.as_str(), &line.amounts).is_some() {
            return Err(RefundAllocationError::InvalidAllocation);
        }
    }
    let shipping_delta = delta(&breakdown(&quote.shipping)?, shipping)?;
    let mut total = shipping_delta.clone();
    let mut allocated = Vec::with_capacity(lines.len());
    for (index, line) in quote.lines.iter().enumerate() {
        let amounts = by_sku
            .remove(line.sku.as_str())
            .ok_or(RefundAllocationError::InvalidAllocation)?;
        let captured = order
            .payable_line_amounts(index)
            .ok_or(RefundAllocationError::InvalidOrder)?;
        let result = delta(&breakdown(captured)?, amounts)?;
        total = sum(&total, &result)?;
        allocated.push((line.sku.clone(), result));
    }
    // Validated quote SKUs are unique; equal cardinality and successful removals
    // consume every input. Each delta is bounded by its captured component, and
    // the validated quote total is the sum of those components.
    if total.gross.minor_units() == 0 {
        return Err(RefundAllocationError::InvalidAllocation);
    }
    Ok(OrderRefundAllocation {
        lines: allocated,
        shipping: shipping_delta,
        total,
    })
}
