//! Guest baskets retain intent only. Publication, money and stock are revalidated at checkout.
use crate::{
    catalogue_authoring::merchant_code,
    media::valid_command_id,
    quotes::{BasketLine, MAX_LINES, MAX_QUANTITY},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BasketUpdate {
    pub expected_revision: i32,
    pub effect_key: String,
    pub lines: Vec<BasketLine>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidBasket;
pub fn valid_lines(lines: &[BasketLine]) -> bool {
    let mut skus = BTreeSet::new();
    lines.len() <= MAX_LINES
        && lines.iter().all(|line| {
            merchant_code(&line.sku)
                && (1..=MAX_QUANTITY).contains(&line.quantity)
                && skus.insert(&line.sku)
        })
}
impl BasketUpdate {
    pub fn canonical_lines(&self) -> Result<Vec<BasketLine>, InvalidBasket> {
        if !(0..i32::MAX).contains(&self.expected_revision)
            || !valid_command_id(&self.effect_key)
            || !valid_lines(&self.lines)
        {
            return Err(InvalidBasket);
        }
        let mut lines = self.lines.clone();
        lines.sort_by(|a, b| a.sku.cmp(&b.sku));
        Ok(lines)
    }
}
