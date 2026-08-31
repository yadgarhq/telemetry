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

/// **The other half of the max, which nothing exercised.** Every case above is
/// bytes-dominant, so `by_bytes.max(by_words)` collapsing to `by_bytes` left
/// the suite green — the words path was decoration.
///
/// Short-worded prose is where words win: 18 bytes over 4.0 is 4.5, while 6
/// words times 1.3 is 7.8. The NUMBER is asserted, because 10 bytes / 2 words —
/// the obvious small case — gives 2.5 against 2.6 and both ceil to 3, which
/// would not discriminate at all.
///
/// Catches: `by_bytes.max(by_words)` becoming `by_bytes`.
#[test]
fn short_words_make_the_word_path_win() {
    let f = Features::of("a cat sat on a mat");
    assert_eq!(f.bytes, 18);
    assert_eq!(f.words, 6);

    let (tokens, _) = estimate(f, Class::Prose);
    assert_eq!(
        tokens, 8,
        "6 words times 1.3 is 7.8, and it beats 18 bytes over 4.0"
    );

    // The same features under a class whose byte floor is higher: bytes win
    // there, which is what makes the pair a test of the max rather than of one
    // branch.
    let (identifiers, _) = estimate(f, Class::Identifiers);
    assert_eq!(identifiers, 48, "6 words times 8.0 beats 18 over 2.5");
}

/// BYTES, not chars, and deliberately so — bytes are what a response costs to
/// transmit and what D49's budget counts. Every other string in this file is
/// ASCII, where the two are identical, so nothing pinned which one it was.
///
/// Catches: `payload.len()` becoming `payload.chars().count()`.
#[test]
fn a_multibyte_payload_is_measured_in_bytes_not_characters() {
    let payload = "naïve café 日本語";
    assert_eq!(payload.chars().count(), 14);

    let f = Features::of(payload);
    assert_eq!(
        f.bytes, 22,
        "two two-byte accents and three three-byte ideographs above the 14 chars"
    );
    assert_eq!(f.words, 3, "words are whitespace-separated, not per glyph");

    // The consequence, as an assertion: a non-English payload of the SAME
    // character count is not the same size on the wire, and the wire is what
    // D49's budget is denominated in.
    let ascii = "naive cafe abc";
    assert_eq!(ascii.chars().count(), payload.chars().count());
    let ascii = Features::of(ascii);
    assert_eq!(ascii.words, f.words);
    assert!(
        f.bytes > ascii.bytes,
        "a multibyte payload costs more to transmit than its ASCII twin of the \
         same length in characters"
    );
}

/// **What makes "VERSION is bumped on ANY coefficient change" enforceable.**
///
/// The existing version test compares the output to the constant that produced
/// it, so it holds for any coefficients whatsoever. This table does not: it is
/// eight independently-written numbers, four classes over both branches of the
/// max, that only this exact set of coefficients produces.
///
/// The loop is what makes the claim bite. Change a coefficient and a row fails,
/// which forces a VERSION bump; bump VERSION and the equality below fails,
/// which forces a new table beside this one. Neither can be done alone.
const GOLDEN_BYTES_WORDS_V1: &[(Class, u64, u32, u32)] = &[
    // class, bytes, words, tokens — bytes-dominant then words-dominant.
    (Class::Prose, 200, 35, 50),
    (Class::Prose, 18, 6, 8),
    (Class::Identifiers, 200, 4, 80),
    (Class::Identifiers, 100, 20, 160),
    (Class::Envelope, 52, 1, 18),
    (Class::Envelope, 60, 10, 26),
    (Class::Mixed, 300, 30, 86),
    (Class::Mixed, 100, 20, 40),
];

#[test]
fn the_golden_table_belongs_to_this_version_and_no_other() {
    assert_eq!(
        VERSION, "bytes-words-v1",
        "a new VERSION needs its own golden table; this one describes \
         bytes-words-v1 and cannot describe a retune"
    );

    for &(class, bytes, words, expected) in GOLDEN_BYTES_WORDS_V1 {
        let f = Features { bytes, words };
        let (tokens, version) = estimate(f, class);
        assert_eq!(
            tokens, expected,
            "{class:?} over {bytes} bytes / {words} words must estimate \
             {expected} under {VERSION}"
        );
        assert_eq!(version, VERSION);
    }
}
