use commerce_domain::{
    holds::{HoldAction::*, HoldError, HoldState, HoldWindow},
    inventory::{StockBalance, StockError},
};

#[test]
fn hold_windows_and_state_transitions_have_exact_deadlines() {
    for (created, expires) in [(-1, 1), (0, 0), (1, 0), (0, 1801)] {
        assert_eq!(
            HoldWindow::new(created, expires),
            Err(HoldError::InvalidWindow)
        );
    }
    HoldWindow::new(1, 1801).expect("maximum TTL");
    HoldWindow::new(i64::MAX - 1, i64::MAX).expect("no overflowing clock arithmetic");
    let window = HoldWindow::new(100, 200).expect("window");
    assert_eq!(
        window.transition(HoldState::Active, Release, 199),
        Ok(HoldState::Released)
    );
    assert_eq!(
        window.transition(HoldState::Active, Consume, 199),
        Ok(HoldState::Consumed)
    );
    assert_eq!(
        window.transition(HoldState::Active, Expire, 200),
        Ok(HoldState::Expired)
    );
    assert_eq!(
        window.transition(HoldState::Active, Consume, 200),
        Err(HoldError::Expired)
    );
    assert_eq!(
        window.transition(HoldState::Active, Expire, 199),
        Err(HoldError::NotExpired)
    );
    assert_eq!(
        window.transition(HoldState::Active, Release, 99),
        Err(HoldError::InvalidClock)
    );
    for (state, replay) in [
        (HoldState::Released, Release),
        (HoldState::Consumed, Consume),
        (HoldState::Expired, Expire),
    ] {
        assert_eq!(window.transition(state, replay, 201), Ok(state));
        for action in [Release, Consume, Expire] {
            if action != replay {
                assert_eq!(
                    window.transition(state, action, 201),
                    Err(HoldError::InvalidTransition)
                );
            }
        }
    }
}

#[test]
fn reservations_release_and_consumption_conserve_physical_components() {
    let before = StockBalance::new(10, 3, 4).expect("valid");
    assert_eq!(before.reserve(2), StockBalance::new(10, 3, 6));
    assert_eq!(before.release(2), StockBalance::new(10, 3, 2));
    assert_eq!(before.consume(2), StockBalance::new(8, 3, 2));
    for invalid in [-1, 0] {
        assert_eq!(before.reserve(invalid), Err(StockError::InvalidQuantity));
        assert_eq!(before.release(invalid), Err(StockError::InvalidQuantity));
        assert_eq!(before.consume(invalid), Err(StockError::InvalidQuantity));
    }
    assert_eq!(before.reserve(4), Err(StockError::InvalidBalance));
    assert_eq!(before.release(5), Err(StockError::InvalidBalance));
    assert_eq!(before.consume(5), Err(StockError::InvalidBalance));
    assert_eq!(
        StockBalance::new(i64::MAX, 0, i64::MAX)
            .expect("limit")
            .reserve(1),
        Err(StockError::Overflow)
    );
    assert_eq!(before.available(), 3);
}
