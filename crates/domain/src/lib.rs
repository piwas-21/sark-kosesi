//! Deterministic catalogue values; HTTP and persistence are adapter concerns.
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub mod catalogue_authoring;
pub mod factual_review;
pub mod holds;
pub mod image_review;
pub mod inventory;
pub mod media;
pub mod promotions;
pub mod publication;
pub mod quotes;
pub mod specifications;
pub mod translations;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Currency {
    EUR,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Money {
    minor_units: i64,
    currency: Currency,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoneyError {
    Negative,
    Overflow,
}

impl Money {
    pub fn eur(minor_units: i64) -> Result<Self, MoneyError> {
        if minor_units < 0 {
            return Err(MoneyError::Negative);
        }
        Ok(Self {
            minor_units,
            currency: Currency::EUR,
        })
    }

    pub fn checked_add(self, other: Self) -> Result<Self, MoneyError> {
        let total = self
            .minor_units
            .checked_add(other.minor_units)
            .ok_or(MoneyError::Overflow)?;
        Self::eur(total)
    }

    pub fn checked_multiply(self, quantity: u32) -> Result<Self, MoneyError> {
        let total = self
            .minor_units
            .checked_mul(i64::from(quantity))
            .ok_or(MoneyError::Overflow)?;
        Self::eur(total)
    }

    pub fn minor_units(self) -> i64 {
        self.minor_units
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogueItem {
    pub sku: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub care: String,
    pub price: Money,
    pub fill: String,
    pub fixture: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentKind {
    Cover,
    Foam,
}
impl ComponentKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::Cover => "cover",
            Self::Foam => "foam",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BomComponent {
    pub code: String,
    pub kind: ComponentKind,
    pub quantity: u32,
}

/// A versioned configuration requirement, never an independent stock pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BillOfMaterials {
    pub sku: String,
    pub revision: i32,
    pub components: Vec<BomComponent>,
}
