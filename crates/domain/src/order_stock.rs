//! Original reservation settlement only. Late allocation and fulfilment are separate.
use crate::{catalogue_authoring::merchant_code, inventory::StockBalance};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub const MAX_MEMBERS: usize = 20;
pub const MAX_LINES: usize = 64;
pub const MAX_PAIRS: usize = MAX_MEMBERS * MAX_LINES;
pub const MAX_FOOTPRINT_BYTES: usize = 1_048_576;
pub const ACTOR: &str = "payment-settlement";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettlementPolicyError {
    InvalidSnapshot,
    InvalidClock,
    RecoveryRequired,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Balance {
    pub on_hand: i64,
    pub quarantine: i64,
    pub reserved: i64,
}
impl Balance {
    pub fn stock(&self) -> Result<StockBalance, SettlementPolicyError> {
        StockBalance::new(self.on_hand, self.quarantine, self.reserved)
            .map_err(|_| SettlementPolicyError::InvalidSnapshot)
    }
    pub fn consume(&self, quantity: i64) -> Result<Self, SettlementPolicyError> {
        let after = self
            .stock()?
            .consume(quantity)
            .map_err(|_| SettlementPolicyError::InvalidSnapshot)?;
        Ok(Self {
            on_hand: after.on_hand(),
            quarantine: after.quarantine(),
            reserved: after.reserved(),
        })
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quantity {
    pub component_code: String,
    pub quantity: i64,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub hold_id: String,
    pub sku: String,
    pub revision: i32,
    pub units: u16,
    pub components: Vec<Quantity>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub component_code: String,
    pub quantity: i64,
    pub before: Balance,
    pub after: Balance,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Footprint {
    pub schema_version: u16,
    pub members: Vec<Member>,
    pub components: Vec<Component>,
}
impl Footprint {
    pub fn demand(members: &[Member]) -> Result<BTreeMap<String, i64>, SettlementPolicyError> {
        let mut demand = BTreeMap::<String, i64>::new();
        let mut holds = BTreeSet::new();
        let mut skus = BTreeSet::new();
        if members.is_empty() || members.len() > MAX_MEMBERS {
            return Err(SettlementPolicyError::InvalidSnapshot);
        }
        for member in members {
            let mut codes = BTreeSet::new();
            if !identifier(&member.hold_id)
                || !holds.insert(&member.hold_id)
                || !merchant_code(&member.sku)
                || !skus.insert(&member.sku)
                || member.revision <= 0
                || !(1..=1000).contains(&member.units)
                || member.components.is_empty()
                || member.components.len() > MAX_LINES
            {
                return Err(SettlementPolicyError::InvalidSnapshot);
            }
            for line in &member.components {
                if !merchant_code(&line.component_code)
                    || !codes.insert(&line.component_code)
                    || !(1..=10_000_000).contains(&line.quantity)
                {
                    return Err(SettlementPolicyError::InvalidSnapshot);
                }
                let total = demand.entry(line.component_code.clone()).or_default();
                *total = total
                    .checked_add(line.quantity)
                    .ok_or(SettlementPolicyError::InvalidSnapshot)?;
            }
        }
        Ok(demand)
    }
    pub fn validate(&self) -> Result<(), SettlementPolicyError> {
        let mut demand = Self::demand(&self.members)?;
        if self.schema_version != 1
            || self.components.len() > MAX_PAIRS
            || self.components.len() != demand.len()
        {
            return Err(SettlementPolicyError::InvalidSnapshot);
        }
        for component in &self.components {
            if demand.remove(&component.component_code) != Some(component.quantity)
                || component.before.consume(component.quantity)? != component.after
            {
                return Err(SettlementPolicyError::InvalidSnapshot);
            }
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settlement {
    pub id: String,
    pub order_id: String,
    pub attempt_id: String,
    pub recognition_id: String,
    pub group_id: String,
    pub actor: String,
    pub settled_at: i64,
    pub footprint: Footprint,
}
impl Settlement {
    pub fn validate(&self) -> Result<(), SettlementPolicyError> {
        if ![
            &self.id,
            &self.order_id,
            &self.attempt_id,
            &self.recognition_id,
            &self.group_id,
        ]
        .into_iter()
        .all(|id| identifier(id))
            || self.actor != ACTOR
            || !(0..=crate::checkout_quotes::MAX_QUOTE_UNIX_SECONDS).contains(&self.settled_at)
        {
            return Err(SettlementPolicyError::InvalidSnapshot);
        }
        self.footprint.validate()
    }
}
pub fn check_time(
    at: i64,
    created: i64,
    expires: i64,
    recognized: i64,
) -> Result<(), SettlementPolicyError> {
    crate::holds::HoldWindow::new(created, expires)
        .map_err(|_| SettlementPolicyError::InvalidSnapshot)?;
    if at < created || at < recognized || at > crate::checkout_quotes::MAX_QUOTE_UNIX_SECONDS {
        return Err(SettlementPolicyError::InvalidClock);
    }
    if at >= expires {
        return Err(SettlementPolicyError::RecoveryRequired);
    }
    Ok(())
}
pub fn identifier(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
