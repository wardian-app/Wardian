use wardian_core::workflow::{normalize, parse_str, to_string, validate};

const PR_REVIEW: &str = include_str!("fixtures/pr-review.md");

#[test]
fn pr_review_parses_validates_and_round_trips() {
    let mut bp = parse_str(PR_REVIEW).expect("parses");
    normalize(&mut bp);

    let report = validate(&bp);
    assert!(report.is_valid(), "diagnostics: {:?}", report.errors());

    let text = to_string(&bp).expect("serializes");
    let mut again = parse_str(&text).expect("re-parses");
    normalize(&mut again);
    assert_eq!(bp, again, "round-trip must be stable after normalization");
}
