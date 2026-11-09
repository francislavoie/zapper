//! What the remuxers' tests share about the things they need from the
//! machine: the ffmpeg command that makes their fixtures, and the decoders
//! and encoders of the FFmpeg they were linked with.
//!
//! On a developer's machine a test without one of those skips, saying why.
//! Where they must all be there it fails instead: under CI (which sets `CI`;
//! it installs ffmpeg and fetches the libmpv bundle the manifest names), or
//! with `ZAPPER_REQUIRE_DECODERS=1`. A bundle that lost a decoder would
//! otherwise turn its tests into skips, and CI would stay green.

/// Whether this run requires every fixture, decoder and encoder.
fn required() -> bool {
    required_by(
        std::env::var_os("CI").is_some(),
        std::env::var("ZAPPER_REQUIRE_DECODERS").ok().as_deref(),
    )
}

fn required_by(ci: bool, require: Option<&str>) -> bool {
    ci || require == Some("1")
}

/// A test can't go on without what `why` names: it skips, saying so, or
/// fails where everything is required.
pub(crate) fn skip(why: &str) {
    skip_unless(why, required());
}

fn skip_unless(why: &str, required: bool) {
    if required {
        panic!("{why}, which this run requires (CI or ZAPPER_REQUIRE_DECODERS=1)");
    }
    eprintln!("skipped: {why}");
}

/// Why a test has no fixture: the ffmpeg command is missing, or failed.
pub(crate) const NO_FIXTURE: &str = "no fixture: the ffmpeg command is missing or failed";

/// Whether the linked FFmpeg decodes `codec` (FFmpeg's name); if not, the
/// test [`skip`]s, or fails where that is required.
pub(crate) fn decoder_or_skip(codec: &str) -> bool {
    if crate::recording::vodmux::can_decode(codec) {
        return true;
    }
    skip(&format!("this FFmpeg has no {codec} decoder"));
    false
}

/// Whether the linked FFmpeg has its AAC encoder; if not, the test
/// [`skip`]s, or fails where that is required.
pub(crate) fn aac_encoder_or_skip() -> bool {
    if crate::recording::livemux::can_encode_aac() {
        return true;
    }
    skip("this FFmpeg has no AAC encoder");
    false
}

/// Removes a fixture's directory, or keeps it and says where when
/// `ZAPPER_KEEP_FIXTURES` is set, to look at the file a test read.
pub(crate) fn keep_or_remove(dir: &std::path::Path) {
    if std::env::var_os("ZAPPER_KEEP_FIXTURES").is_some() {
        eprintln!("fixture kept in {}", dir.display());
    } else {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_or_the_variable_requires_everything() {
        assert!(!required_by(false, None));
        assert!(required_by(true, None), "CI, whatever its value");
        assert!(required_by(false, Some("1")));
        assert!(!required_by(false, Some("0")));
    }

    #[test]
    fn a_skip_only_prints_where_nothing_is_required() {
        skip_unless("this FFmpeg has no mov_text decoder", false);
    }

    #[test]
    #[should_panic(expected = "this FFmpeg has no mov_text decoder, which this run requires")]
    fn a_skip_fails_where_everything_is_required() {
        skip_unless("this FFmpeg has no mov_text decoder", true);
    }
}
