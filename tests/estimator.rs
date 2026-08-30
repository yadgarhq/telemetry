//! The estimator. Pure, and the one part of this crate with logic that can be
//! wrong in a way nobody notices.

use yadgar_telemetry::estimator::{error_ratio, estimate, Class, Features, VERSION};

/// The point of recording both: a UUID is one word and many tokens, prose is one
/// word and roughly one token. Same word count, wildly different cost — which is
/// exactly what a single global ratio cannot express.
#[test]
fn identifiers_cost_more_than_prose_of_the_same_size() {
    // Realistic densities: prose is many short words, URNs are few long ones.
    let prose = Features {
        bytes: 200,
        words: 35,
    };
    let ids = Features {
        bytes: 200,
        words: 4,
    };
    let (p, _) = estimate(prose, Class::Prose);
    let (i, _) = estimate(ids, Class::Identifiers);
    assert!(
        i > p,
        "200 bytes of URNs must cost more than 200 bytes of prose: {i} vs {p}"
    );
}

/// A JSON blob with no whitespace has bytes and zero words. Estimating from
/// words would return 0 — the most wrong a number can be, and silently so.
#[test]
fn a_payload_with_no_whitespace_is_not_free() {
    let f = Features::of(r#"{"id":"yadgar:task:01a052c9","version":1,"status":5}"#);
    assert_eq!(f.words, 1, "no spaces means one 'word'");
    let (tokens, _) = estimate(f, Class::Envelope);
    assert!(
        tokens > 5,
        "a 50-byte envelope is not 2 tokens: got {tokens}"
    );
}

/// The truly wordless case: bytes present, zero words.
#[test]
fn zero_words_falls_back_to_bytes_rather_than_returning_zero() {
    let f = Features {
        bytes: 400,
        words: 0,
    };
    let (tokens, _) = estimate(f, Class::Prose);
    assert!(tokens > 0, "400 bytes must not estimate as zero tokens");
}

/// Retuning changes what every past number means, so an estimate that does not
/// carry its version cannot be compared to anything.
#[test]
fn every_estimate_carries_its_version() {
    let (_, v) = estimate(
        Features {
            bytes: 10,
            words: 2,
        },
        Class::Prose,
    );
    assert_eq!(v, VERSION);
    assert!(!v.is_empty(), "an unlabelled estimate is uncomparable");
}

/// Bytes per word is a signal in its own right, not a step toward one: a high
/// ratio says trim the envelope, which is a different fix from returning less.
#[test]
fn bytes_per_word_separates_envelope_heavy_from_content_heavy() {
    let prose = Features::of("the quick brown fox jumps over the lazy dog again");
    let urns = Features::of("yadgar:task:01a052c9-271a-7873-bd5f-dca4913cf22a yadgar:task:01a052d2-f535-75a0-8afb-77df741a5f08");
    assert!(
        urns.bytes_per_word().unwrap() > prose.bytes_per_word().unwrap() * 2.0,
        "URNs must show a far higher bytes-per-word than prose"
    );
}

#[test]
fn an_empty_payload_has_no_ratio_rather_than_dividing_by_zero() {
    assert!(Features::of("").bytes_per_word().is_none());
}

/// D67: the estimator's own error is an exported metric. Without it, "we will
/// tune it for accuracy" is an intention rather than a plan.
#[test]
fn error_against_ground_truth_is_measurable() {
    assert_eq!(error_ratio(100, 100), Some(1.0));
    assert_eq!(error_ratio(200, 100), Some(2.0));
    assert_eq!(error_ratio(50, 100), Some(0.5));
    assert_eq!(
        error_ratio(100, 0),
        None,
        "no ground truth means no error figure"
    );
}

/// The default class is Mixed, not Prose. A payload nobody classified must not
/// be assumed to be the cheapest kind — that under-reports it silently, which is
/// the direction that makes a response look cheaper than it is.
#[test]
fn the_default_class_is_conservative() {
    assert_eq!(Class::default(), Class::Mixed);
    let f = Features {
        bytes: 300,
        words: 30,
    };
    let (mixed, _) = estimate(f, Class::default());
    let (prose, _) = estimate(f, Class::Prose);
    assert!(mixed > prose, "the default must not be the cheapest class");
}
