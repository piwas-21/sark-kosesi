use commerce_application::guest_access::*;
#[test]
fn lifetime_has_fixed_inclusive_start_exclusive_expiry_and_checked_bounds() {
    let live = GuestLifetime {
        created_at: 1000,
        expires_at: 605800,
    };
    assert!(live.valid());
    assert_eq!(live.check(1000), Ok(()));
    assert_eq!(live.check(605799), Ok(()));
    for at in [-1, 999, 605800, i64::MAX] {
        assert_eq!(live.check(at), Err(GuestPolicyError::Expired));
    }
    for (created_at, expires_at) in [(-1, 1), (0, 0), (10, 9), (0, 604801), (i64::MIN, i64::MAX)] {
        let invalid = GuestLifetime {
            created_at,
            expires_at,
        };
        assert!(!invalid.valid());
        assert_eq!(invalid.check(1), Err(GuestPolicyError::InvalidState));
    }
    assert!(
        GuestLifetime {
            created_at: i64::MAX - 1,
            expires_at: i64::MAX
        }
        .valid()
    );
}
#[test]
fn guest_resource_limits_cannot_be_zero_or_unbounded() {
    assert!(GuestLimits::default().valid());
    for (issue_per_minute, mutations_per_minute) in [(1, 1), (1000, 1000)] {
        assert!(
            GuestLimits {
                issue_per_minute,
                mutations_per_minute
            }
            .valid()
        );
    }
    for (issue_per_minute, mutations_per_minute) in [(0, 1), (1001, 1), (1, 0), (1, 1001)] {
        assert!(
            !GuestLimits {
                issue_per_minute,
                mutations_per_minute
            }
            .valid()
        );
    }
}
