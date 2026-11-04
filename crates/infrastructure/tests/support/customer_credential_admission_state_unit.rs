use super::*;

fn initialized() -> Scope {
    Scope {
        window_start: Some(960),
        last_seen_at: Some(1000),
        key_version: Some(2),
        key_check: Some(vec![7; 32]),
        global_limit: Some(10),
        target_limit: Some(3),
        admitted: 4,
    }
}

fn empty() -> Scope {
    Scope {
        window_start: None,
        last_seen_at: None,
        key_version: None,
        key_check: None,
        global_limit: None,
        target_limit: None,
        admitted: 0,
    }
}

fn unavailable(scope: &Scope, case: &str) {
    assert!(
        matches!(scope.validate(), Err(AdmissionError::Unavailable)),
        "{case} must fail closed"
    );
}

#[test]
fn accepts_empty_and_complete_scope_with_exact_stored_values() {
    assert!(matches!(empty().validate(), Ok(None)));
    let scope = initialized();
    let previous = match scope.validate() {
        Ok(Some(previous)) => previous,
        _ => panic!("complete scope must validate"),
    };
    assert_eq!(previous.window_start, 960);
    assert_eq!(previous.last_seen_at, 1000);
    assert_eq!(previous.key_version, 2);
    assert_eq!(previous.key_check, vec![7; 32]);
    assert!(previous.limits == AdmissionLimits::new(10, 3).expect("valid limits"));
}

#[test]
fn accepts_inclusive_window_limit_and_counter_boundaries() {
    let mut scope = initialized();
    scope.window_start = Some(0);
    scope.last_seen_at = Some(0);
    scope.key_version = Some(1);
    scope.global_limit = Some(1);
    scope.target_limit = Some(1);
    scope.admitted = 0;
    assert!(matches!(scope.validate(), Ok(Some(_))));
    scope.last_seen_at = Some(59);
    scope.admitted = 1;
    assert!(matches!(scope.validate(), Ok(Some(_))));

    scope.window_start = Some(MAX_QUOTE_UNIX_SECONDS - MAX_QUOTE_UNIX_SECONDS % WINDOW);
    scope.last_seen_at = Some(MAX_QUOTE_UNIX_SECONDS);
    scope.key_version = Some(i32::MAX);
    scope.global_limit = Some(1000);
    scope.target_limit = Some(100);
    scope.admitted = 1000;
    assert!(matches!(scope.validate(), Ok(Some(_))));
}

#[test]
fn rejects_uninitialized_counts_and_each_partially_null_field() {
    for count in [-1, 1] {
        let mut scope = empty();
        scope.admitted = count;
        unavailable(&scope, "uninitialized nonzero count");
    }
    let mut cases = Vec::new();
    let mut scope = initialized();
    scope.window_start = None;
    cases.push(("missing window", scope));
    let mut scope = initialized();
    scope.last_seen_at = None;
    cases.push(("missing last observation", scope));
    let mut scope = initialized();
    scope.key_version = None;
    cases.push(("missing key version", scope));
    let mut scope = initialized();
    scope.key_check = None;
    cases.push(("missing key check", scope));
    let mut scope = initialized();
    scope.global_limit = None;
    cases.push(("missing global limit", scope));
    let mut scope = initialized();
    scope.target_limit = None;
    cases.push(("missing target limit", scope));
    for (case, scope) in cases {
        unavailable(&scope, case);
    }
}

#[test]
fn rejects_invalid_limits_and_independently_damaged_counter() {
    for (global, target) in [
        (-1, 1),
        (0, 1),
        (1001, 1),
        (10, 0),
        (10, -1),
        (101, 101),
        (2, 3),
    ] {
        let mut scope = initialized();
        scope.global_limit = Some(global);
        scope.target_limit = Some(target);
        unavailable(&scope, "invalid limits");
    }
    for count in [-1, 11] {
        let mut scope = initialized();
        scope.admitted = count;
        unavailable(&scope, "count outside stored global limit");
    }
}

#[test]
fn rejects_invalid_window_key_and_last_observation_independently() {
    for window in [-60, 961, MAX_QUOTE_UNIX_SECONDS + 1] {
        let mut scope = initialized();
        scope.window_start = Some(window);
        unavailable(&scope, "invalid window start");
    }
    for last in [959, 1020, MAX_QUOTE_UNIX_SECONDS + 1] {
        let mut scope = initialized();
        scope.last_seen_at = Some(last);
        unavailable(&scope, "invalid last observation");
    }
    for version in [-1, 0] {
        let mut scope = initialized();
        scope.key_version = Some(version);
        unavailable(&scope, "invalid key version");
    }
    for length in [0, 31, 33] {
        let mut scope = initialized();
        scope.key_check = Some(vec![7; length]);
        unavailable(&scope, "invalid key check length");
    }
}
