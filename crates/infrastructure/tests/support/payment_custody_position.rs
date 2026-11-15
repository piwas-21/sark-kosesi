use super::*;

#[test]
fn payment_custody_preflight_checkpoint_boundaries() {
    use Phase::{Candidates, NeedsReview, Poll, Search};
    for (phase, index, page, allowed) in [
        (Search, 0, None, true),
        (Search, 1, None, false),
        (Candidates, 0, Some((1, false, 1)), true),
        (Candidates, 1, Some((1, false, 1)), true),
        (Candidates, 2, Some((1, false, 1)), false),
        (NeedsReview, 0, Some((1, true, 100)), true),
        (NeedsReview, 1, Some((1, true, 100)), true),
        (Search, 1, Some((1, true, 99)), true),
        (Search, 0, Some((1, true, 99)), false),
        (Search, 1, Some((1, false, 99)), false),
        (Search, 1, Some((1, true, 100)), false),
        (Poll, 1, Some((1, false, 1)), true),
        (Poll, 0, Some((1, false, 1)), false),
        (Poll, 1, Some((1, true, 1)), false),
    ] {
        assert_eq!(checkpoint_position(phase, index, page).is_ok(), allowed);
    }
}
