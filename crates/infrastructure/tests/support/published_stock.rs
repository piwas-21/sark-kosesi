use super::*;
#[test]
fn incomplete_corrupt_or_overflowing_physical_snapshots_are_never_sellable() {
    let line = BomLineDraft {
        component_code: "COVER".into(),
        quantity: 2,
    };
    let row = || Balance {
        component_code: "COVER".into(),
        quantity: 2,
        on_hand: Some(4),
        quarantine: Some(1),
        reserved: Some(1),
    };
    assert!(available_rows(vec![row()], std::slice::from_ref(&line)).expect("physical available"));
    for (rows, lines) in [
        (vec![], vec![]),
        (vec![], vec![line.clone()]),
        (vec![row()], vec![]),
        (
            vec![row()],
            vec![BomLineDraft {
                component_code: "OTHER".into(),
                quantity: 2,
            }],
        ),
        (
            vec![row()],
            vec![BomLineDraft {
                component_code: "COVER".into(),
                quantity: 3,
            }],
        ),
    ] {
        assert!(matches!(
            available_rows(rows, &lines),
            Err(PublishedError::InvalidStoredData)
        ));
    }
    let mut r = row();
    r.quantity = 0;
    assert!(matches!(
        available_rows(vec![r], &[line]),
        Err(PublishedError::InvalidStoredData)
    ));
    for (on_hand, quarantine, reserved) in [
        (Some(-1), Some(0), Some(0)),
        (Some(i64::MAX), Some(i64::MAX), Some(1)),
        (None, Some(0), None),
        (Some(1), None, Some(0)),
        (None, None, Some(0)),
    ] {
        let r = Balance {
            on_hand,
            quarantine,
            reserved,
            ..row()
        };
        assert!(matches!(
            r.available(),
            Err(PublishedError::InvalidStoredData)
        ));
    }
    assert_eq!(
        Balance {
            on_hand: None,
            quarantine: None,
            reserved: None,
            ..row()
        }
        .available()
        .expect("unreceived"),
        0
    );
}
