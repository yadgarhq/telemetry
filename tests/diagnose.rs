//! What an operator READS when something refuses.
//!
//! The unit under test is a formatter, so the only assertion worth making is on
//! the EXACT text. A test that checked "the detail is non-empty", or that it
//! `contains` the head of the chain, would pass for `to_string()` alone — which
//! is precisely the code this function replaces, and precisely the bug it fixes.
//!
//! The `": "` separator is an interface to something outside this repository:
//! five services put the result of this function into an error message a human
//! reads in a crash loop. Changing it silently reformats all five, so it is
//! pinned by a literal here rather than by reading the constant back.

use std::error::Error;
use std::fmt;

use yadgar_telemetry::diagnose::chain;

/// A link in a synthetic chain, so the test owns every layer of it.
///
/// Built by hand rather than with `thiserror`: the point is to control what
/// `Display` and `source` each return independently, and a derive that ties
/// them together would make the test agree with itself instead of with the
/// contract.
#[derive(Debug)]
struct Link {
    message: &'static str,
    source: Option<Box<Link>>,
}

impl Link {
    fn head(message: &'static str) -> Self {
        Self {
            message,
            source: None,
        }
    }

    fn over(message: &'static str, source: Link) -> Self {
        Self {
            message,
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Display for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl Error for Link {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref().map(|link| link as &dyn Error)
    }
}

/// The shape this function exists for: `tonic::transport::Error` displays as
/// three useless words and keeps the reason underneath it.
#[test]
fn every_layer_of_the_chain_is_in_the_sentence() {
    let error = Link::over(
        "transport error",
        Link::over(
            "invalid private key",
            Link::head("no PKCS#8 object found in the input"),
        ),
    );

    assert_eq!(
        chain(&error),
        "transport error: invalid private key: no PKCS#8 object found in the input"
    );
}

/// An error with nothing under it renders as itself and stops.
///
/// The failure this pins is a trailing `": "` on every message that has no
/// source, which is most of them.
#[test]
fn an_error_with_no_source_gains_nothing() {
    assert_eq!(chain(&Link::head("permission denied")), "permission denied");
}

/// Two layers, so the separator is asserted at a boundary the three-deep case
/// could not distinguish from a join over the whole list.
#[test]
fn the_separator_between_two_layers_is_a_colon_and_a_space() {
    let error = Link::over("cannot read /etc/yadgar/tls/tls.crt", Link::head("ENOENT"));

    assert_eq!(chain(&error), "cannot read /etc/yadgar/tls/tls.crt: ENOENT");
}

/// An error borrowing something that does not outlive the program.
///
/// `Link` above cannot stand in for this: it holds `&'static str` and a `Box`,
/// so `Link: 'static` and it coerces to `&(dyn Error + 'static)` just as
/// happily. A test built on it would pass under EITHER signature and pin
/// neither — the certifying-fixture shape, which is what this suite exists to
/// avoid. `Borrowed` carries a lifetime, so it coerces to `&'a (dyn Error + 'a)`
/// and to nothing stricter.
#[derive(Debug)]
struct Borrowed<'a>(&'a str);

impl fmt::Display for Borrowed<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for Borrowed<'_> {}

/// The argument is a plain `&dyn Error`, so a borrowed non-`'static` error is
/// accepted rather than refused.
///
/// This is a COMPILE-TIME assertion wearing a test's clothes: the copies this
/// function consolidates disagreed on the signature — three took
/// `&dyn Error`, two took `&(dyn Error + 'static)` — and the permissive one is
/// what got published. Tighten `chain` to the strict form and this stops
/// compiling, which was measured rather than assumed:
/// `error[E0597]: `owned` does not live long enough`.
#[test]
fn a_borrowed_error_is_accepted() {
    let owned = String::from("held by a local");
    let error = Borrowed(&owned);
    let borrowed: &dyn Error = &error;

    assert_eq!(chain(borrowed), "held by a local");
}
