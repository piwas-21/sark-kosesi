use super::Error;
use commerce_domain::{
    catalogue_authoring::merchant_code, order_shipment_components::expected_components,
    order_shipments::SkuUnits,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Units {
    pub sku: String,
    pub units: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub code: String,
    pub quantity: i64,
}
pub type Packages = Vec<Vec<Component>>;
#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow)]
pub struct Record {
    pub id: String,
    pub order_id: String,
    pub attempt_id: String,
    pub recognition_id: String,
    pub allocation_kind: String,
    pub allocation_id: String,
    pub actor_id: String,
    pub effect_key: String,
    pub generation: i16,
    pub action: String,
    pub target_id: Option<String>,
    pub units: sqlx::types::Json<Vec<Units>>,
    pub packages: sqlx::types::Json<Packages>,
    pub recorded_at: i64,
    pub event_id: String,
}
pub fn check_packages(packages: &[Vec<Component>]) -> Result<BTreeMap<String, i64>, Error> {
    if !(1..=32).contains(&packages.len()) {
        return Err(Error::InvalidRequest);
    }
    let mut result = BTreeMap::<String, i64>::new();
    for package in packages {
        if !(1..=64).contains(&package.len()) {
            return Err(Error::InvalidRequest);
        }
        let mut seen = BTreeSet::new();
        for part in package {
            if !merchant_code(&part.code)
                || !(1..=1_000_000_000).contains(&part.quantity)
                || !seen.insert(&part.code)
            {
                return Err(Error::InvalidRequest);
            }
            let value = result.entry(part.code.clone()).or_default();
            *value = value
                .checked_add(part.quantity)
                .ok_or(Error::InvalidRequest)?;
        }
    }
    Ok(result)
}
fn quantities(items: &[Units]) -> Vec<SkuUnits> {
    items
        .iter()
        .map(|u| SkuUnits {
            sku: u.sku.clone(),
            units: u.units,
        })
        .collect()
}
/// Validate every record and reconstruct active occupancy, including released packs.
pub fn validate(
    source: &crate::shipment_planning::Source,
    records: &[Record],
) -> Result<(Vec<SkuUnits>, usize), Error> {
    let mut active = BTreeMap::<&str, Vec<Units>>::new();
    let mut previous_time = 0;
    let mut prior: Vec<_> = source
        .order
        .quote
        .lines
        .iter()
        .map(|l| SkuUnits {
            sku: l.sku.clone(),
            units: 0,
        })
        .collect();
    for (index, record) in records.iter().enumerate() {
        if usize::try_from(record.generation).ok() != Some(index + 1)
            || record.order_id != source.order.identity.id
            || record.allocation_kind != source.allocation_kind
            || record.allocation_id != source.allocation_id
            || record.recorded_at < previous_time
        {
            return Err(Error::InvalidStoredData);
        }
        if record.action == "packed" {
            let proposal = quantities(&record.units);
            let expected = expected_components(
                &source.order,
                &prior,
                &proposal,
                &source.members,
                &source.boms,
            )
            .map_err(|_| Error::InvalidStoredData)?;
            let actual = check_packages(&record.packages).map_err(|_| Error::InvalidStoredData)?;
            let expected: BTreeMap<_, _> =
                expected.into_iter().map(|x| (x.code, x.quantity)).collect();
            if actual != expected
                || record.target_id.is_some()
                || active.insert(&record.id, record.units.0.clone()).is_some()
            {
                return Err(Error::InvalidStoredData);
            }
        } else if record.action == "released" {
            let id = record
                .target_id
                .as_deref()
                .ok_or(Error::InvalidStoredData)?;
            if !record.units.is_empty()
                || !record.packages.is_empty()
                || active.remove(id).is_none()
            {
                return Err(Error::InvalidStoredData);
            }
        } else {
            return Err(Error::InvalidStoredData);
        }
        for line in &mut prior {
            line.units = 0;
            for units in active.values() {
                if let Some(u) = units.iter().find(|u| u.sku == line.sku) {
                    line.units = line
                        .units
                        .checked_add(u.units)
                        .ok_or(Error::InvalidStoredData)?;
                }
            }
        }
        if index + 1 + active.len() > 64 {
            return Err(Error::InvalidStoredData);
        }
        previous_time = record.recorded_at;
    }
    Ok((prior, active.len()))
}
