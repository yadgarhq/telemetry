//! Turning bytes and words into a token estimate (D67).
//!
//! **This is an estimator, not a tokenizer, and the distinction is the design.**
//! Vendoring a real BPE tokenizer means shipping one model's vocabulary: correct
//! for that model, wrong for every other caller, and stale when the vendor
//! changes it. An estimator over cheap features costs no vocabulary and is
//! model-agnostic by construction.
//!
//! **Accuracy comes from fitting per content class, not from one global ratio.**
//! The whole problem is that identifiers and prose tokenize differently, and
//! yadgar knows which it just returned. A `recall` returning URNs and a
//! `wiki_read` returning markdown can carry the same word count and cost wildly
//! different amounts.
//!
//! **Every estimate carries the version that produced it.** Retuning changes what
//! every historical number means, so without the version a dashboard silently
//! compares this month against last month under a different model.

/// Bumped on ANY coefficient change. Recorded on every estimate, so a retune is
/// a visible discontinuity in the data rather than an invisible one.
pub const VERSION: &str = "bytes-words-v1";

/// What kind of payload this is, because one ratio cannot serve all of them.
///
/// Deliberately coarse: these are the shapes yadgar actually returns, not a
/// taxonomy of text. A class splits when a measurement shows it should, and the
/// version changes when it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Sentences. The case where words track tokens most closely.
    Prose,
    /// URNs, UUIDs, slugs, hex. A UUID is one word and roughly fifteen tokens,
    /// so a word count under-reports this class badly.
    Identifiers,
    /// JSON or protobuf-shaped output: field names, punctuation, structure.
    /// Tokenizes heavily while barely registering as words.
    Envelope,
    /// Mixed, or not yet classified. The conservative default.
    Mixed,
}

/// Bytes per token, per class — the FLOOR.
///
/// Words alone are not enough, and the failure is silent: a JSON envelope with no
/// whitespace is one "word" and fifty bytes, so a word-only estimate returns
/// three tokens for something that costs twenty. Identifiers tokenize into short
/// pieces (2-3 bytes each), prose into longer ones (~4).
///
/// The estimate takes whichever of the two paths is LARGER. Under-reporting is
/// the dangerous direction here: it makes a response look cheaper than it is,
/// which is the opposite of what the measurement exists to catch.
const fn bytes_per_token(class: Class) -> f32 {
    match class {
        Class::Prose => 4.0,
        Class::Identifiers => 2.5,
        Class::Envelope => 3.0,
        Class::Mixed => 3.5,
    }
}

/// Tokens per word, per class.
///
/// PROVISIONAL, and labelled as such rather than presented as measured. The
/// prose figure (~1.3) is the well-known English average; the others are
/// reasoned from structure and are the first thing calibration should replace
/// once `ask` starts producing ground truth. `error_ratio` below is what makes
/// that replacement evidence-driven rather than a guess about a guess.
const fn tokens_per_word(class: Class) -> f32 {
    match class {
        Class::Prose => 1.3,
        Class::Identifiers => 8.0,
        Class::Envelope => 2.6,
        Class::Mixed => 2.0,
    }
}

/// Cheap features over a payload. No allocation, one pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Features {
    pub bytes: u64,
    pub words: u32,
}

impl Features {
    /// Count bytes and whitespace-separated words.
    ///
    /// Bytes, not chars: bytes are what the response actually costs to transmit
    /// and what D49's budget is denominated in.
    pub fn of(payload: &str) -> Self {
        Self {
            bytes: payload.len() as u64,
            words: payload.split_whitespace().count() as u32,
        }
    }

    /// Bytes per word — a signal in its own right, not a step toward one.
    ///
    /// A high ratio means an identifier- and envelope-heavy payload, which calls
    /// for trimming the envelope rather than returning less content. Those are
    /// different fixes and one number cannot tell them apart.
    pub fn bytes_per_word(&self) -> Option<f32> {
        (self.words > 0).then(|| self.bytes as f32 / self.words as f32)
    }
}

/// Estimate tokens for a payload of a known class.
///
/// Returns the estimate and [`VERSION`]; the two are never separated, because an
/// estimate without its version cannot be compared to anything.
pub fn estimate(features: Features, class: Class) -> (u32, &'static str) {
    // A payload with bytes but no words is pure structure — a JSON blob with no
    // whitespace, or one long identifier. Words would say zero and the estimate
    // would be zero, which is the most wrong a number can be. Fall back to bytes.
    let by_bytes = features.bytes as f32 / bytes_per_token(class);
    let by_words = features.words as f32 * tokens_per_word(class);

    // The LARGER of the two. Each path fails in one direction: words under-report
    // dense, whitespace-free payloads (a URN or a JSON blob is one "word"), and
    // bytes over-report nothing much but are the safer floor. Taking the max
    // means an unusual payload is estimated by whichever feature actually saw it.
    (by_bytes.max(by_words).ceil() as u32, VERSION)
}

/// How wrong the estimate was, given ground truth.
///
/// **The estimator's own error is a metric (D67).** If nobody can say how wrong
/// it is, "we will tune it for accuracy" is an intention rather than a plan —
/// which is the objection D15 raises to every rule that cannot be observed.
/// Ground truth comes from `ask`, which gets real token counts back from LLM
/// providers as a side effect of doing its job.
///
/// Returns estimated/actual: 1.0 is perfect, 2.0 is double, 0.5 is half.
pub fn error_ratio(estimated: u32, actual: u32) -> Option<f32> {
    (actual > 0).then(|| estimated as f32 / actual as f32)
}
