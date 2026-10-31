use commerce_domain::{guest_baskets::*, quotes::BasketLine};
fn line(sku: &str, quantity: u32) -> BasketLine {
    BasketLine {
        sku: sku.into(),
        quantity,
    }
}
fn update(lines: Vec<BasketLine>) -> BasketUpdate {
    BasketUpdate {
        expected_revision: 0,
        effect_key: "one".into(),
        lines,
    }
}
#[test]
fn only_bounded_unique_merchant_intent_can_be_canonicalized() {
    assert!(update(vec![]).canonical_lines().expect("empty").is_empty());
    assert_eq!(
        update(vec![line("Z-A", 100), line("A", 1)])
            .canonical_lines()
            .expect("canonical"),
        vec![line("A", 1), line("Z-A", 100)]
    );
    assert!(valid_lines(
        &(0..20)
            .map(|n| line(&format!("SKU-{n}"), 1))
            .collect::<Vec<_>>()
    ));
    for lines in [
        vec![line("A", 0)],
        vec![line("A", 101)],
        vec![line("A", u32::MAX)],
        vec![line("a", 1)],
        vec![line("-A", 1)],
        vec![line("A-", 1)],
        vec![line("DEMO", 1)],
        vec![line("DEMO-A", 1)],
        vec![line("A", 1), line("A", 2)],
        (0..21).map(|n| line(&format!("A-{n}"), 1)).collect(),
    ] {
        assert!(!valid_lines(&lines));
        assert!(update(lines).canonical_lines().is_err());
    }
}
#[test]
fn effect_and_revision_are_closed_and_cannot_overflow() {
    for revision in [-1, i32::MAX] {
        let mut r = update(vec![]);
        r.expected_revision = revision;
        assert!(r.canonical_lines().is_err());
    }
    for key in ["", "bad/key", &"a".repeat(129)] {
        let mut r = update(vec![]);
        r.effect_key = key.into();
        assert!(r.canonical_lines().is_err());
    }
    let mut r = update(vec![]);
    r.expected_revision = i32::MAX - 1;
    r.effect_key = "a".repeat(128);
    assert!(r.canonical_lines().is_ok());
}
