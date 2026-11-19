//! Versions and the lines they fall in. See "Versions" in
//! `docs/spec/releases.md`.

use std::fmt;

pub use semver::Version;

/// The versions that are compatible with one another: those with one
/// major; under major zero, with one minor as well; and under minor
/// zero too, one version alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Line {
    major: u64,
    /// Said only under major zero.
    minor: Option<u64>,
    /// Said only under major and minor zero.
    patch: Option<u64>,
}

impl Line {
    /// The line `version` is in.
    #[must_use]
    pub fn of(version: &Version) -> Self {
        let minor = (version.major == 0).then_some(version.minor);
        let patch = (minor == Some(0)).then_some(version.patch);
        Self {
            major: version.major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.major)?;
        for part in [self.minor, self.patch].into_iter().flatten() {
            write!(f, ".{part}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Line, Version};

    fn line(text: &str) -> String {
        let version = Version::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"));
        Line::of(&version).to_string()
    }

    #[test]
    fn a_line_is_a_version_as_far_as_its_first_number_that_is_not_zero() {
        assert_eq!(line("1.2.3"), "1");
        assert_eq!(line("12.0.0"), "12");
        assert_eq!(line("0.2.3"), "0.2");
        assert_eq!(line("0.0.3"), "0.0.3");
        assert_eq!(line("0.0.0"), "0.0.0");
    }

    #[test]
    fn a_pre_release_is_in_the_line_of_what_it_comes_before() {
        assert_eq!(line("2.0.0-rc.1"), "2");
        assert_eq!(line("0.3.0-alpha+build.5"), "0.3");
    }
}
