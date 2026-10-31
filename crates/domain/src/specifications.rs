//! Bounded supplier/measurement drafts. Validation never establishes factual approval.
use crate::{BillOfMaterials, ComponentKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DimensionsMm {
    pub width_mm: u32,
    pub depth_mm: u32,
    pub height_mm: u32,
    pub tolerance_mm: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FibrePart {
    pub name: String,
    pub basis_points: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum MaterialSpecification {
    Cover {
        fibres: Vec<FibrePart>,
        care_evidence_ref: String,
    },
    Foam {
        density_g_per_m3: u32,
        firmness_evidence_ref: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentSpecification {
    pub component_code: String,
    pub dimensions: DimensionsMm,
    pub material: MaterialSpecification,
    pub source_ref: String,
    pub traceability_ref: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParcelMeasurements {
    pub dimensions: DimensionsMm,
    pub gross_weight_g: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecificationDraft {
    pub footprint: DimensionsMm,
    pub assembly_evidence_ref: String,
    pub components: Vec<ComponentSpecification>,
    pub parcels: Vec<ParcelMeasurements>,
    pub packaging_evidence_ref: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecificationError {
    Measurements,
    Materials,
    Evidence,
    Components,
    Parcels,
}
pub fn bounded_text(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= maximum
}
impl DimensionsMm {
    pub(crate) fn validate(&self) -> Result<(), SpecificationError> {
        for value in [self.width_mm, self.depth_mm, self.height_mm] {
            if !(1..=10000).contains(&value) || self.tolerance_mm >= value {
                return Err(SpecificationError::Measurements);
            }
        }
        if self.tolerance_mm > 500 {
            return Err(SpecificationError::Measurements);
        }
        Ok(())
    }
}
impl MaterialSpecification {
    fn validate(&self) -> Result<ComponentKind, SpecificationError> {
        match self {
            Self::Cover {
                fibres,
                care_evidence_ref,
            } => {
                if fibres.is_empty() || fibres.len() > 16 {
                    return Err(SpecificationError::Materials);
                }
                let mut names = BTreeSet::new();
                let mut total = 0_u32;
                for part in fibres {
                    if !bounded_text(&part.name, 80)
                        || !(1..=10000).contains(&part.basis_points)
                        || !names.insert(part.name.trim().to_lowercase())
                    {
                        return Err(SpecificationError::Materials);
                    }
                    total = total
                        .checked_add(u32::from(part.basis_points))
                        .ok_or(SpecificationError::Materials)?;
                }
                if total != 10000 {
                    return Err(SpecificationError::Materials);
                }
                if !bounded_text(care_evidence_ref, 512) {
                    return Err(SpecificationError::Evidence);
                }
                Ok(ComponentKind::Cover)
            }
            Self::Foam {
                density_g_per_m3,
                firmness_evidence_ref,
            } => {
                if !(1..=1000000).contains(density_g_per_m3) {
                    return Err(SpecificationError::Materials);
                }
                if !bounded_text(firmness_evidence_ref, 512) {
                    return Err(SpecificationError::Evidence);
                }
                Ok(ComponentKind::Foam)
            }
        }
    }
}
impl SpecificationDraft {
    /// Bounds are an input sanity envelope, not product or carrier claims.
    pub fn validate(&self, bom: &BillOfMaterials) -> Result<(), SpecificationError> {
        self.footprint.validate()?;
        if !bounded_text(&self.assembly_evidence_ref, 512)
            || !bounded_text(&self.packaging_evidence_ref, 512)
        {
            return Err(SpecificationError::Evidence);
        }
        if self.components.is_empty()
            || self.components.len() > 64
            || self.components.len() != bom.components.len()
        {
            return Err(SpecificationError::Components);
        }
        let mut codes = BTreeSet::new();
        for component in &self.components {
            if !codes.insert(&component.component_code) {
                return Err(SpecificationError::Components);
            }
            let Some(required) = bom
                .components
                .iter()
                .find(|line| line.code == component.component_code)
            else {
                return Err(SpecificationError::Components);
            };
            component.dimensions.validate()?;
            if component.material.validate()? != required.kind {
                return Err(SpecificationError::Components);
            }
            if !bounded_text(&component.source_ref, 512)
                || !bounded_text(&component.traceability_ref, 512)
            {
                return Err(SpecificationError::Evidence);
            }
        }
        if self.parcels.is_empty() || self.parcels.len() > 64 {
            return Err(SpecificationError::Parcels);
        }
        for parcel in &self.parcels {
            parcel.dimensions.validate()?;
            if !(1..=1000000).contains(&parcel.gross_weight_g) {
                return Err(SpecificationError::Parcels);
            }
        }
        Ok(())
    }
}
