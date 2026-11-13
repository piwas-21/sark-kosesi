//! Whole-variant quantity accounting only; no shipment or stock authority.
use crate::{catalogue_authoring::merchant_code, checkout_orders::OrderSnapshot};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShipmentQuantityError {
    InvalidOrder,
    InvalidQuantities,
}

/// A caller-supplied count of complete sellable variant units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkuUnits {
    pub sku: String,
    pub units: u32,
}

/// One accepted quote line, in its original order. No parcel is described here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShipmentLineQuantities {
    pub sku: String,
    pub ordered: u32,
    pub prior: u32,
    pub proposed: u32,
    pub remaining: u32,
}

/// Check a proposal against all previously accounted whole units.
///
/// `prior` must name every accepted SKU once, even when its count is zero.
/// `proposed` names one or more distinct accepted SKUs with positive counts.
/// Callers must authenticate and reconstruct complete prior shipment custody
/// under their own transaction; these supplied values are not verified history.
/// A successful result does not grant permission to pick, dispatch or consume
/// stock, and does not establish that a multipart variant has been shipped.
pub fn plan_units(
    order: &OrderSnapshot,
    prior: &[SkuUnits],
    proposed: &[SkuUnits],
) -> Result<Vec<ShipmentLineQuantities>, ShipmentQuantityError> {
    order
        .validate()
        .map_err(|_| ShipmentQuantityError::InvalidOrder)?;
    let lines = &order.quote.lines;
    if prior.len() != lines.len() || proposed.is_empty() || proposed.len() > lines.len() {
        return Err(ShipmentQuantityError::InvalidQuantities);
    }
    let mut prior_by_sku = BTreeMap::new();
    for item in prior {
        if !merchant_code(&item.sku) || prior_by_sku.insert(item.sku.as_str(), item.units).is_some()
        {
            return Err(ShipmentQuantityError::InvalidQuantities);
        }
    }
    let mut proposal_by_sku = BTreeMap::new();
    for item in proposed {
        if !merchant_code(&item.sku)
            || item.units == 0
            || proposal_by_sku
                .insert(item.sku.as_str(), item.units)
                .is_some()
        {
            return Err(ShipmentQuantityError::InvalidQuantities);
        }
    }
    let mut result = Vec::with_capacity(lines.len());
    for line in lines {
        let previous = prior_by_sku
            .remove(line.sku.as_str())
            .ok_or(ShipmentQuantityError::InvalidQuantities)?;
        let requested = proposal_by_sku.remove(line.sku.as_str()).unwrap_or(0);
        let total = previous
            .checked_add(requested)
            .ok_or(ShipmentQuantityError::InvalidQuantities)?;
        let remaining = line
            .quantity
            .checked_sub(total)
            .ok_or(ShipmentQuantityError::InvalidQuantities)?;
        result.push(ShipmentLineQuantities {
            sku: line.sku.clone(),
            ordered: line.quantity,
            prior: previous,
            proposed: requested,
            remaining,
        });
    }
    if !proposal_by_sku.is_empty() {
        return Err(ShipmentQuantityError::InvalidQuantities);
    }
    Ok(result)
}
