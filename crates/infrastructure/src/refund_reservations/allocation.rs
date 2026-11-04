use super::*;
use commerce_domain::{
    Money,
    order_refunds::{RefundAmounts, SkuRefund, allocate_order_refund},
    quotes::AmountBreakdown,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefundAllocationAmounts {
    pub gross_minor: i64,
    pub net_minor: i64,
    pub vat_minor: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefundAllocationLine {
    pub sku: String,
    pub gross_minor: i64,
    pub net_minor: i64,
    pub vat_minor: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefundAllocation {
    pub schema_version: u8,
    pub lines: Vec<RefundAllocationLine>,
    pub shipping: RefundAllocationAmounts,
    pub total: RefundAllocationAmounts,
}
fn amounts(value: &AmountBreakdown) -> RefundAllocationAmounts {
    RefundAllocationAmounts {
        gross_minor: value.gross.minor_units(),
        net_minor: value.net.minor_units(),
        vat_minor: value.vat.minor_units(),
    }
}
fn money(value: i64) -> Result<Money, RefundError> {
    Money::eur(value).map_err(|_| RefundError::InvalidRequest)
}
fn sum(a: i64, b: i64) -> Result<i64, RefundError> {
    a.checked_add(b).ok_or(RefundError::InvalidStoredData)
}
fn compute(
    order: &OrderSnapshot,
    history: &[RefundReceipt],
    lines: &[RefundLine<'_>],
    shipping: i64,
) -> Result<RefundAllocation, RefundError> {
    let mut balances: BTreeMap<&str, i64> = BTreeMap::new();
    let mut shipped = 0;
    for receipt in history {
        for line in &receipt.allocation.lines {
            let existing = balances.entry(&line.sku).or_insert(0);
            *existing = sum(*existing, line.gross_minor)?;
        }
        shipped = sum(shipped, receipt.allocation.shipping.gross_minor)?;
    }
    let input = lines
        .iter()
        .map(|line| {
            Ok(SkuRefund {
                sku: line.sku.into(),
                amounts: RefundAmounts {
                    already_refunded_gross: money(*balances.get(line.sku).unwrap_or(&0))?,
                    requested_gross: money(line.gross_minor)?,
                },
            })
        })
        .collect::<Result<Vec<_>, RefundError>>()?;
    let shipping = RefundAmounts {
        already_refunded_gross: money(shipped)?,
        requested_gross: money(shipping)?,
    };
    let result =
        allocate_order_refund(order, &input, &shipping).map_err(|_| RefundError::InvalidRequest)?;
    Ok(RefundAllocation {
        schema_version: 1,
        lines: result
            .lines
            .iter()
            .map(|(sku, a)| RefundAllocationLine {
                sku: sku.clone(),
                gross_minor: a.gross.minor_units(),
                net_minor: a.net.minor_units(),
                vat_minor: a.vat.minor_units(),
            })
            .collect(),
        shipping: amounts(&result.shipping),
        total: amounts(&result.total),
    })
}
pub(super) fn calculate(
    order: &OrderSnapshot,
    history: &[RefundReceipt],
    request: &RefundRequest<'_>,
) -> Result<RefundAllocation, RefundError> {
    compute(order, history, request.lines, request.shipping_gross_minor)
}
pub(super) fn same_request(allocation: &RefundAllocation, request: &RefundRequest<'_>) -> bool {
    if allocation.lines.len() != request.lines.len()
        || allocation.shipping.gross_minor != request.shipping_gross_minor
    {
        return false;
    }
    let mut requested = BTreeMap::new();
    for line in request.lines {
        if requested.insert(line.sku, line.gross_minor).is_some() {
            return false;
        }
    }
    allocation
        .lines
        .iter()
        .all(|line| requested.remove(line.sku.as_str()) == Some(line.gross_minor))
        && requested.is_empty()
}
pub(super) fn validate_history(
    order: &OrderSnapshot,
    attempt: &str,
    history: &[RefundReceipt],
) -> Result<(), RefundError> {
    for (index, receipt) in history.iter().enumerate() {
        if receipt.order_id != order.identity.id
            || receipt.attempt_id != attempt
            || usize::try_from(receipt.generation).ok() != Some(index + 1)
            || (index > 0 && receipt.reserved_at < history[index - 1].reserved_at)
        {
            return Err(RefundError::InvalidStoredData);
        }
        if receipt.reserved_at < order.identity.created_at {
            return Err(RefundError::InvalidStoredData);
        }
        let lines = receipt
            .allocation
            .lines
            .iter()
            .map(|l| RefundLine {
                sku: &l.sku,
                gross_minor: l.gross_minor,
            })
            .collect::<Vec<_>>();
        let expected = compute(
            order,
            &history[..index],
            &lines,
            receipt.allocation.shipping.gross_minor,
        )
        .map_err(|_| RefundError::InvalidStoredData)?;
        if expected != receipt.allocation {
            return Err(RefundError::InvalidStoredData);
        }
    }
    Ok(())
}
