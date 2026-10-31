//! Literal identity/paid evidence oracles; no status winner or financial effects.
#[path = "support/payment_observation_fixture.rs"]
mod fixture;
use commerce_domain::payment_attempts::{identities::*, *};
use fixture::*;
#[test]
fn unknown_single_and_ambiguous_identities_preserve_every_paid_fact() {
    let a = attempt();
    let empty = summarize(&a, 1200, &[]).expect("empty evidence");
    assert!(empty.provider_ids.is_empty());
    assert!(empty.paid_provider_ids.is_empty());
    let mut one = observation();
    one.intent.payment.id = "tr_B".into();
    one.intent.payment.status = PaymentStatus::Pending;
    let s = summarize(&a, 1200, &[one.clone()]).expect("single");
    assert_eq!(s.provider_ids, vec!["tr_B"]);
    assert!(s.paid_provider_ids.is_empty());
    let mut paid = one.clone();
    paid.revision = 2;
    paid.intent.payment.status = PaymentStatus::Paid;
    let mut old = one.clone();
    old.revision = 3;
    let s = summarize(&a, 1200, &[one.clone(), paid.clone(), old.clone()]).expect("reordered");
    assert_eq!(s.provider_ids, vec!["tr_B"]);
    assert_eq!(s.paid_provider_ids, vec!["tr_B"]);
    let mut other = paid.clone();
    other.revision = 4;
    other.intent.payment.id = "tr_A".into();
    let s = summarize(&a, 1200, &[one, paid, old, other]).expect("ambiguous");
    assert_eq!(s.provider_ids, vec!["tr_A", "tr_B"]);
    assert_eq!(s.paid_provider_ids, vec!["tr_A", "tr_B"]);
}
#[test]
fn full_contiguous_monotonic_original_binding_and_bounds_are_required() {
    let a = attempt();
    let one = observation();
    let mut gap = one.clone();
    gap.revision = 3;
    assert!(summarize(&a, 1200, &[one.clone(), gap]).is_err());
    let mut rewind = one.clone();
    rewind.revision = 2;
    rewind.recorded_at = 4999;
    assert!(summarize(&a, 1200, &[one.clone(), rewind]).is_err());
    let mut wrong = one.clone();
    wrong.intent.payment.amount_minor = 1;
    assert!(summarize(&a, 1200, &[wrong]).is_err());
    assert!(summarize(&a, 2000, &[]).is_err());
    let mut bounded = vec![];
    for n in 1..=1000 {
        let mut o = one.clone();
        o.revision = n;
        bounded.push(o);
    }
    assert_eq!(
        summarize(&a, 1200, &bounded)
            .expect("exact bound")
            .paid_provider_ids
            .len(),
        1
    );
    bounded.push(one);
    assert!(summarize(&a, 1200, &bounded).is_err());
}
