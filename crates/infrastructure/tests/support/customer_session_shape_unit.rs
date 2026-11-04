use super::{SessionError, SessionRow, now};
use commerce_application::request_context::{Clock, ContextError};

fn active_row() -> SessionRow {
    SessionRow {
        account_id: 1,
        credential_revision: 1,
        created_at: 1000,
        expires_at: 44200,
        rotated_from: None,
        revoked_at: None,
        revocation_kind: None,
        created_xid: "42".to_owned(),
        revoked_xid_empty: true,
    }
}

#[test]
fn session_row_rejects_invalid_identity_lifetime_and_revocation() {
    let valid = active_row();
    assert!(valid.active(1000));
    assert!(valid.active(44199));
    assert!(!valid.active(999));
    assert!(!valid.active(44200));

    let mut row = active_row();
    row.account_id = 0;
    assert!(!row.active(1000));
    let mut row = active_row();
    row.credential_revision = 0;
    assert!(!row.active(1000));
    let mut row = active_row();
    row.created_at = -1;
    assert!(!row.active(1000));
    let mut row = active_row();
    row.expires_at = 1000;
    assert!(!row.active(1000));
    let mut row = active_row();
    row.expires_at = 44201;
    assert!(!row.active(1000));
    let mut row = active_row();
    row.rotated_from = Some(vec![0; 31]);
    assert!(!row.active(1000));
    row.rotated_from = Some(vec![0; 32]);
    assert!(row.active(1000));
    let mut row = active_row();
    row.created_xid.clear();
    assert!(!row.active(1000));
    let mut row = active_row();
    row.revoked_xid_empty = false;
    assert!(!row.active(1000));
    let mut row = active_row();
    row.revoked_at = Some(1000);
    assert!(!row.active(1000));
    let mut row = active_row();
    row.revocation_kind = Some("logout".to_owned());
    assert!(!row.active(1000));
}

struct FixedClock(Result<u64, ContextError>);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        self.0
    }
}

#[test]
fn clock_failure_and_unrepresentable_time_are_unavailable() {
    assert_eq!(now(&FixedClock(Ok(1000))), Ok(1000));
    assert_eq!(
        now(&FixedClock(Ok(u64::MAX))),
        Err(SessionError::Unavailable)
    );
    assert_eq!(
        now(&FixedClock(Err(ContextError::ClockUnavailable))),
        Err(SessionError::Unavailable)
    );
}
