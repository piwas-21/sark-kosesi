//! Expected component quantities only; no warehouse or shipment authority.
use crate::{
    BillOfMaterials,
    catalogue_authoring::{Fill, matches_fill, merchant_code},
    checkout_orders::OrderSnapshot,
    order_shipments::{ShipmentQuantityError, SkuUnits, plan_units},
    order_stock::{Footprint, Member},
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShipmentComponentError {
    Quantities(ShipmentQuantityError),
    InvalidFootprint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedComponent {
    pub code: String,
    pub quantity: i64,
}

/// Project complete proposed variant units to expected physical components.
///
/// Both the original sealed BOMs and the full frozen allocation members are
/// caller-supplied. This checks their mutual consistency with the accepted
/// order, but does not authenticate them or the supplied prior shipment counts.
/// It creates no package, stock, shipment, payment, or dispatch authority.
pub fn expected_components(
    order: &OrderSnapshot,
    prior: &[SkuUnits],
    proposed: &[SkuUnits],
    members: &[Member],
    boms: &[BillOfMaterials],
) -> Result<Vec<ExpectedComponent>, ShipmentComponentError> {
    let plan = plan_units(order, prior, proposed).map_err(ShipmentComponentError::Quantities)?;
    Footprint::demand(members).map_err(|_| ShipmentComponentError::InvalidFootprint)?;
    if members.len() != plan.len() || boms.len() != plan.len() {
        return Err(ShipmentComponentError::InvalidFootprint);
    }
    let mut by_sku = BTreeMap::new();
    for bom in boms {
        if !merchant_code(&bom.sku)
            || bom.revision <= 0
            || bom.components.is_empty()
            || bom.components.len() > crate::order_stock::MAX_LINES
            || by_sku.insert(bom.sku.as_str(), bom).is_some()
        {
            return Err(ShipmentComponentError::InvalidFootprint);
        }
    }
    let mut expected = BTreeMap::<&str, i64>::new();
    for ((quantity, line), product) in plan.iter().zip(&order.quote.lines).zip(&order.products) {
        let member = members
            .iter()
            .find(|member| member.sku == line.sku)
            .ok_or(ShipmentComponentError::InvalidFootprint)?;
        let bom = by_sku
            .remove(line.sku.as_str())
            .ok_or(ShipmentComponentError::InvalidFootprint)?;
        if member.revision != line.bom_revision
            || bom.revision != line.bom_revision
            || u32::from(member.units) != line.quantity
            || bom.components.len() != member.components.len()
        {
            return Err(ShipmentComponentError::InvalidFootprint);
        }
        let fill = match product.fill.as_str() {
            "covers-only" => Fill::CoversOnly,
            "with-foam" => Fill::WithFoam,
            _ => return Err(ShipmentComponentError::InvalidFootprint),
        };
        let mut components = BTreeMap::new();
        let mut kinds = Vec::with_capacity(bom.components.len());
        for component in &bom.components {
            if !merchant_code(&component.code)
                || !(1..=10_000).contains(&component.quantity)
                || components
                    .insert(component.code.as_str(), component.quantity)
                    .is_some()
            {
                return Err(ShipmentComponentError::InvalidFootprint);
            }
            kinds.push(component.kind);
        }
        if !matches_fill(fill, &kinds) {
            return Err(ShipmentComponentError::InvalidFootprint);
        }
        for component in &member.components {
            let units = components
                .remove(component.component_code.as_str())
                .ok_or(ShipmentComponentError::InvalidFootprint)?;
            let total = i64::from(units)
                .checked_mul(i64::from(member.units))
                .ok_or(ShipmentComponentError::InvalidFootprint)?;
            if total != component.quantity {
                return Err(ShipmentComponentError::InvalidFootprint);
            }
            if quantity.proposed != 0 {
                let requested = i64::from(units)
                    .checked_mul(i64::from(quantity.proposed))
                    .ok_or(ShipmentComponentError::InvalidFootprint)?;
                let value = expected
                    .entry(component.component_code.as_str())
                    .or_default();
                *value = value
                    .checked_add(requested)
                    .ok_or(ShipmentComponentError::InvalidFootprint)?;
            }
        }
        if !components.is_empty() {
            return Err(ShipmentComponentError::InvalidFootprint);
        }
    }
    if !by_sku.is_empty() {
        return Err(ShipmentComponentError::InvalidFootprint);
    }
    Ok(expected
        .into_iter()
        .map(|(code, quantity)| ExpectedComponent {
            code: code.to_owned(),
            quantity,
        })
        .collect())
}
