use commerce_domain::inventory::{StockBalance, StockError, WarehouseChange::*};

#[test]
fn warehouse_transitions_preserve_physical_units_with_independent_oracles() {
    let balance = StockBalance::new(10, 3, 2).expect("valid");
    assert_eq!(balance.available(), 5);
    for (change, expected, descriptor) in [
        (Receive(4), (14, 3, 2, 9), ("receive", 4)),
        (Quarantine(4), (10, 7, 2, 1), ("quarantine", 4)),
        (
            ReleaseQuarantine(2),
            (10, 1, 2, 7),
            ("release-quarantine", 2),
        ),
        (
            DisposeQuarantine(2),
            (8, 1, 2, 5),
            ("dispose-quarantine", 2),
        ),
        (Count(7), (7, 3, 2, 2), ("count", 7)),
    ] {
        let actual = balance.warehouse_change(change).expect("valid change");
        assert_eq!(
            (
                actual.on_hand(),
                actual.quarantine(),
                actual.reserved(),
                actual.available()
            ),
            expected
        );
        assert_eq!(change.descriptor(), descriptor);
    }
    assert_eq!(
        StockBalance::new(0, 0, 0)
            .expect("empty")
            .warehouse_change(Count(0))
            .expect("zero stocktake")
            .available(),
        0
    );
    assert_eq!(
        StockBalance::new(i64::MAX, i64::MAX, 0)
            .expect("limit")
            .available(),
        0
    );
}

#[test]
fn negatives_scarcity_and_overflow_fail_without_changing_the_balance() {
    for (on_hand, quarantine, reserved, error) in [
        (-1, 0, 0, StockError::InvalidBalance),
        (0, -1, 0, StockError::InvalidBalance),
        (0, 0, -1, StockError::InvalidBalance),
        (1, 1, 1, StockError::InvalidBalance),
        (i64::MAX, i64::MAX, 1, StockError::Overflow),
    ] {
        assert_eq!(StockBalance::new(on_hand, quarantine, reserved), Err(error));
    }
    let balance = StockBalance::new(10, 3, 2).expect("valid");
    for change in [
        Receive(0),
        Quarantine(0),
        ReleaseQuarantine(0),
        DisposeQuarantine(0),
        Count(-1),
    ] {
        assert_eq!(
            balance.warehouse_change(change),
            Err(StockError::InvalidQuantity)
        );
    }
    for change in [
        Quarantine(6),
        ReleaseQuarantine(4),
        DisposeQuarantine(4),
        Count(4),
    ] {
        assert_eq!(
            balance.warehouse_change(change),
            Err(StockError::InvalidBalance)
        );
    }
    let maximum = StockBalance::new(i64::MAX, i64::MAX, 0).expect("maximum");
    assert_eq!(
        maximum.warehouse_change(Receive(1)),
        Err(StockError::Overflow)
    );
    assert_eq!(
        maximum.warehouse_change(Quarantine(1)),
        Err(StockError::Overflow)
    );
    assert_eq!(balance.available(), 5);
}
