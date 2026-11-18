//! Captured commercial amounts only: no invoice, payment or tax authority.
use crate::{checkout_orders::OrderSnapshot, checkout_quotes::CapturedAmount};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasisLine {
    pub sku: String,
    pub quantity: u32,
    pub unit_gross_minor: i64,
    pub vat_basis_points: u16,
    pub original: CapturedAmount,
    pub discount_minor: i64,
    pub payable: CapturedAmount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasisShipping {
    pub vat_basis_points: u16,
    pub original: CapturedAmount,
    pub payable: CapturedAmount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RateBasis {
    pub vat_basis_points: u16,
    pub original: CapturedAmount,
    pub discount_minor: i64,
    pub payable: CapturedAmount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderFinancialBasis {
    pub currency: String,
    pub lines: Vec<BasisLine>,
    pub shipping: BasisShipping,
    /// Sorted by rate; amounts are summed from captured components, not re-rounded.
    pub rates: Vec<RateBasis>,
    pub original_total: CapturedAmount,
    pub discount_minor: i64,
    pub payable_total: CapturedAmount,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BasisError {
    InvalidOrder,
}

fn zero() -> CapturedAmount {
    CapturedAmount {
        gross_minor: 0,
        net_minor: 0,
        vat_minor: 0,
    }
}

fn add(left: &mut CapturedAmount, right: &CapturedAmount) -> Result<(), BasisError> {
    left.gross_minor = left
        .gross_minor
        .checked_add(right.gross_minor)
        .ok_or(BasisError::InvalidOrder)?;
    left.net_minor = left
        .net_minor
        .checked_add(right.net_minor)
        .ok_or(BasisError::InvalidOrder)?;
    left.vat_minor = left
        .vat_minor
        .checked_add(right.vat_minor)
        .ok_or(BasisError::InvalidOrder)?;
    Ok(())
}

fn accumulate(
    rates: &mut BTreeMap<u16, RateBasis>,
    rate: u16,
    original: &CapturedAmount,
    discount: i64,
    payable: &CapturedAmount,
) -> Result<(), BasisError> {
    let entry = rates.entry(rate).or_insert_with(|| RateBasis {
        vat_basis_points: rate,
        original: zero(),
        discount_minor: 0,
        payable: zero(),
    });
    add(&mut entry.original, original)?;
    add(&mut entry.payable, payable)?;
    entry.discount_minor = entry
        .discount_minor
        .checked_add(discount)
        .ok_or(BasisError::InvalidOrder)?;
    Ok(())
}

/// Validate and project the captured order, without asserting historical provenance.
/// A caller reading persisted data must independently establish trusted source custody
/// and access authority. This result asserts neither payment nor fiscal acceptance.
pub fn order_financial_basis(order: &OrderSnapshot) -> Result<OrderFinancialBasis, BasisError> {
    order.validate().map_err(|_| BasisError::InvalidOrder)?;
    let mut rates = BTreeMap::new();
    let mut lines = Vec::with_capacity(order.quote.lines.len());
    for (index, line) in order.quote.lines.iter().enumerate() {
        let payable = order
            .payable_line_amounts(index)
            .ok_or(BasisError::InvalidOrder)?
            .clone();
        let discount = match &order.promoted_quote {
            Some(promotion) => {
                promotion
                    .lines
                    .get(index)
                    .ok_or(BasisError::InvalidOrder)?
                    .discount_minor
            }
            None => 0,
        };
        accumulate(
            &mut rates,
            line.vat_basis_points,
            &line.amounts,
            discount,
            &payable,
        )?;
        lines.push(BasisLine {
            sku: line.sku.clone(),
            quantity: line.quantity,
            unit_gross_minor: line.unit_gross_minor,
            vat_basis_points: line.vat_basis_points,
            original: line.amounts.clone(),
            discount_minor: discount,
            payable,
        });
    }
    let shipping = BasisShipping {
        vat_basis_points: order.quote.shipping_vat_basis_points,
        original: order.quote.shipping.clone(),
        payable: order.quote.shipping.clone(),
    };
    accumulate(
        &mut rates,
        shipping.vat_basis_points,
        &shipping.original,
        0,
        &shipping.payable,
    )?;
    let mut original_total = zero();
    let mut payable_total = zero();
    let mut discount_minor = 0_i64;
    for rate in rates.values() {
        if rate.original.gross_minor.checked_sub(rate.discount_minor)
            != Some(rate.payable.gross_minor)
        {
            return Err(BasisError::InvalidOrder);
        }
        add(&mut original_total, &rate.original)?;
        add(&mut payable_total, &rate.payable)?;
        discount_minor = discount_minor
            .checked_add(rate.discount_minor)
            .ok_or(BasisError::InvalidOrder)?;
    }
    if original_total != order.quote.total
        || payable_total != *order.payable_total()
        || original_total.gross_minor.checked_sub(discount_minor) != Some(payable_total.gross_minor)
    {
        return Err(BasisError::InvalidOrder);
    }
    Ok(OrderFinancialBasis {
        currency: order.quote.currency.clone(),
        lines,
        shipping,
        rates: rates.into_values().collect(),
        original_total,
        discount_minor,
        payable_total,
    })
}
