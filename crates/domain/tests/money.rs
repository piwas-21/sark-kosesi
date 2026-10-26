use commerce_domain::{Money, MoneyError};

#[test]
fn exact_minor_units_with_independent_oracles() {
    let unit = Money::eur(12_345).expect("fixture is valid");
    assert_eq!(
        unit.checked_multiply(3).expect("bounded").minor_units(),
        37_035
    );
    assert_eq!(
        unit.checked_add(Money::eur(55).expect("valid"))
            .expect("bounded")
            .minor_units(),
        12_400
    );
    assert_eq!(unit.checked_multiply(0).expect("zero").minor_units(), 0);
    assert_eq!(Money::eur(0).expect("zero").minor_units(), 0);
}

#[test]
fn negative_and_overflow_fail_without_wrapping() {
    assert_eq!(Money::eur(-1), Err(MoneyError::Negative));
    let max = Money::eur(i64::MAX).expect("valid bound");
    assert_eq!(
        max.checked_add(Money::eur(1).expect("valid")),
        Err(MoneyError::Overflow)
    );
    assert_eq!(max.checked_multiply(2), Err(MoneyError::Overflow));
    assert_eq!(max.checked_add(Money::eur(0).expect("valid")), Ok(max));
    assert_eq!(max.checked_multiply(1), Ok(max));
}
