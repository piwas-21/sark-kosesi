//! Closed object/array shapes and source bounds are checked before typed allocation.
use super::*;
use serde_json::Value;
fn array<'a>(value: &'a Value, key: &str, maximum: usize) -> Result<&'a [Value], LateStockError> {
    let array = value
        .get(key)
        .and_then(Value::as_array)
        .ok_or(LateStockError::InvalidStoredData)?;
    if array.is_empty() || array.len() > maximum {
        return Err(LateStockError::InvalidStoredData);
    }
    Ok(array)
}
fn member(member: &Value) -> Result<(), LateStockError> {
    if !member.is_object() {
        return Err(LateStockError::InvalidStoredData);
    }
    if array(
        member,
        "components",
        commerce_domain::order_stock::MAX_LINES,
    )?
    .iter()
    .any(|line| !line.is_object())
    {
        return Err(LateStockError::InvalidStoredData);
    }
    Ok(())
}
fn shape(value: &Value, late: bool) -> Result<(), LateStockError> {
    if !value.is_object() {
        return Err(LateStockError::InvalidStoredData);
    }
    for original in array(value, "original", commerce_domain::order_stock::MAX_MEMBERS)? {
        if !original.is_object() {
            return Err(LateStockError::InvalidStoredData);
        }
        member(
            original
                .get("member")
                .ok_or(LateStockError::InvalidStoredData)?,
        )?;
        if array(original, "pairs", commerce_domain::order_stock::MAX_LINES)?
            .iter()
            .any(|pair| !pair.is_object())
        {
            return Err(LateStockError::InvalidStoredData);
        }
    }
    if late {
        for fresh in array(value, "fresh", commerce_domain::order_stock::MAX_MEMBERS)? {
            member(fresh)?;
        }
    }
    for component in array(value, "components", commerce_domain::order_stock::MAX_PAIRS)? {
        if !component.is_object() || !component.get("before").is_some_and(Value::is_object) {
            return Err(LateStockError::InvalidStoredData);
        }
        if late
            && ["expired", "reserved", "after"]
                .iter()
                .any(|key| !component.get(*key).is_some_and(Value::is_object))
        {
            return Err(LateStockError::InvalidStoredData);
        }
    }
    Ok(())
}
pub(super) fn allocation(value: Value) -> Result<LateFootprint, LateStockError> {
    shape(&value, true)?;
    serde_json::from_value(value).map_err(|_| LateStockError::InvalidStoredData)
}
pub(super) fn review(value: Value) -> Result<ReviewFootprint, LateStockError> {
    shape(&value, false)?;
    serde_json::from_value(value).map_err(|_| LateStockError::InvalidStoredData)
}
