use commerce_application::staff_access::{AccessDenied, Capability, SessionState, check_session};
fn valid() -> SessionState {
    SessionState {
        enabled: true,
        revoked: false,
        granted: true,
        created_at: 1000,
        expires_at: 4600,
        mfa_verified_at: Some(1100),
    }
}

#[test]
fn exact_session_and_recent_mfa_boundaries_are_required() {
    assert_eq!(check_session(valid(), 1100), Ok(()));
    assert_eq!(check_session(valid(), 2000), Ok(()));
    assert_eq!(
        check_session(
            SessionState {
                expires_at: 44200,
                ..valid()
            },
            2000
        ),
        Ok(())
    );
    for (state, now) in [
        (
            SessionState {
                enabled: false,
                ..valid()
            },
            1100,
        ),
        (
            SessionState {
                revoked: true,
                ..valid()
            },
            1100,
        ),
        (
            SessionState {
                granted: false,
                ..valid()
            },
            1100,
        ),
        (
            SessionState {
                created_at: -1,
                ..valid()
            },
            1100,
        ),
        (
            SessionState {
                expires_at: 1000,
                ..valid()
            },
            1100,
        ),
        (
            SessionState {
                expires_at: 44201,
                ..valid()
            },
            1100,
        ),
        (valid(), 999),
        (valid(), 4600),
        (
            SessionState {
                mfa_verified_at: None,
                ..valid()
            },
            1100,
        ),
        (
            SessionState {
                mfa_verified_at: Some(999),
                ..valid()
            },
            1100,
        ),
        (
            SessionState {
                mfa_verified_at: Some(1101),
                ..valid()
            },
            1100,
        ),
        (valid(), 2001),
    ] {
        assert_eq!(check_session(state, now), Err(AccessDenied));
    }
    assert_eq!(Capability::CatalogueManage.code(), "catalogue.manage");
    assert_eq!(Capability::WarehouseManage.code(), "warehouse.manage");
}
