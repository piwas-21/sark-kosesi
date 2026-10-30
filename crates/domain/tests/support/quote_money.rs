use super::*;
#[test]
fn money_boundaries_reject_negative_wide_and_addition_overflow() {
    for value in [-1, MAX_MINOR_UNITS + 1] {
        assert_eq!(money(value), Err(QuoteError::AmountTooLarge));
    }
    for value in [i128::MAX, i128::MIN, i128::from(MAX_MINOR_UNITS) + 1] {
        assert_eq!(wide_money(value), Err(QuoteError::AmountTooLarge));
    }
    let max = Money::eur(i64::MAX).expect("bound");
    let one = Money::eur(1).expect("one");
    assert_eq!(add(max, one), Err(QuoteError::AmountTooLarge));
    assert_eq!(
        add(money(MAX_MINOR_UNITS).expect("exact"), one),
        Err(QuoteError::AmountTooLarge)
    );
    assert_eq!(
        add(money(2).expect("two"), one).expect("sum").minor_units(),
        3
    );
    assert_eq!(wide_money(3).expect("wide exact").minor_units(), 3);
}
