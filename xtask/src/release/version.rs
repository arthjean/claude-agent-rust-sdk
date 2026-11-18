//! Semantic versions as the release commands read, compare, and bump them:
//! `MAJOR.MINOR.PATCH` with an optional pre-release part, no build
//! metadata.

use std::cmp::Ordering;
use std::fmt;

/// One version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// `MAJOR`.
    pub major: u64,
    /// `MINOR`.
    pub minor: u64,
    /// `PATCH`.
    pub patch: u64,
    /// The dot-separated identifiers after `-`, empty for a release.
    pub pre: Vec<String>,
}

fn number(part: &str, text: &str) -> Result<u64, String> {
    let leading_zero = part.len() > 1 && part.starts_with('0');
    if part.is_empty() || leading_zero || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("`{text}` is not a version of the form X.Y.Z[-pre]"));
    }
    part.parse()
        .map_err(|_| format!("`{text}` is not a version of the form X.Y.Z[-pre]"))
}

impl Version {
    /// Parses `X.Y.Z` or `X.Y.Z-pre`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let invalid = || format!("`{text}` is not a version of the form X.Y.Z[-pre]");
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        let [major, minor, patch] = parts.as_slice() else {
            return Err(invalid());
        };
        let pre: Vec<String> = match pre {
            None => Vec::new(),
            Some(pre) => {
                let identifiers: Vec<String> = pre.split('.').map(str::to_owned).collect();
                let valid = identifiers.iter().all(|identifier| {
                    !identifier.is_empty()
                        && identifier
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                        && !(identifier.len() > 1
                            && identifier.starts_with('0')
                            && identifier.bytes().all(|byte| byte.is_ascii_digit()))
                });
                if !valid {
                    return Err(invalid());
                }
                identifiers
            }
        };
        Ok(Self {
            major: number(major, text)?,
            minor: number(minor, text)?,
            patch: number(patch, text)?,
            pre,
        })
    }

    /// Whether the version has a pre-release part.
    pub fn is_pre_release(&self) -> bool {
        !self.pre.is_empty()
    }

    /// Whether this is a pre-release of `release`: `X.Y.Z-pre` for the
    /// release `X.Y.Z`.
    pub fn is_pre_release_of(&self, release: &Self) -> bool {
        self.is_pre_release()
            && !release.is_pre_release()
            && (self.major, self.minor, self.patch) == (release.major, release.minor, release.patch)
    }

    /// The next patch version: the release of a pre-release, or `PATCH + 1`.
    pub fn next_patch(&self) -> Self {
        let patch = if self.is_pre_release() {
            self.patch
        } else {
            self.patch + 1
        };
        Self {
            patch,
            pre: Vec::new(),
            ..self.clone()
        }
    }

    /// The next minor version: the release of a pre-release of `X.Y.0`, or
    /// `X.(Y + 1).0`.
    pub fn next_minor(&self) -> Self {
        let minor = if self.is_pre_release() && self.patch == 0 {
            self.minor
        } else {
            self.minor + 1
        };
        Self {
            major: self.major,
            minor,
            patch: 0,
            pre: Vec::new(),
        }
    }

    /// `MAJOR.MINOR`, the requirement a `Cargo.toml` snippet gives.
    pub fn major_minor(&self) -> String {
        format!("{}.{}", self.major, self.minor)
    }
}

fn compare_identifiers(left: &str, right: &str) -> Ordering {
    match (left.parse::<u64>(), right.parse::<u64>()) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => left.cmp(right),
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (left, right) in self.pre.iter().zip(&other.pre) {
                        let order = compare_identifiers(left, right);
                        if order != Ordering::Equal {
                            return order;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if self.is_pre_release() {
            write!(formatter, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Version;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn versions_round_trip_and_reject_malformed_text() {
        for text in ["0.1.0", "1.22.333", "0.1.0-rc.1", "2.0.0-alpha.beta-2"] {
            assert_eq!(version(text).to_string(), text);
        }
        for text in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.2.x",
            "v1.2.3",
            "1.2.3-",
            "1.2.3-rc..1",
            "1.2.3-01",
            "1.2.3+build",
        ] {
            assert!(Version::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn pre_releases_sort_before_their_release_and_numerically_among_themselves() {
        let ordered = [
            "0.0.9",
            "0.1.0-alpha",
            "0.1.0-rc.1",
            "0.1.0-rc.2",
            "0.1.0-rc.10",
            "0.1.0",
            "0.1.1",
            "0.2.0",
            "1.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(version(pair[0]) < version(pair[1]), "{pair:?}");
        }
    }

    #[test]
    fn bumps_release_a_pre_release_or_step_the_number() {
        assert_eq!(version("0.1.2").next_patch().to_string(), "0.1.3");
        assert_eq!(version("0.1.0-rc.2").next_patch().to_string(), "0.1.0");
        assert_eq!(version("0.1.2").next_minor().to_string(), "0.2.0");
        assert_eq!(version("0.2.0-rc.1").next_minor().to_string(), "0.2.0");
        assert_eq!(version("0.2.1-rc.1").next_minor().to_string(), "0.3.0");
        assert_eq!(version("0.2.1-rc.1").major_minor(), "0.2");
    }

    #[test]
    fn a_pre_release_belongs_only_to_the_release_with_its_numbers() {
        let release = version("0.1.0");
        assert!(version("0.1.0-rc.1").is_pre_release_of(&release));
        assert!(version("0.1.0-alpha").is_pre_release_of(&release));
        assert!(!version("0.1.1-rc.1").is_pre_release_of(&release));
        assert!(!version("0.0.9").is_pre_release_of(&release));
        assert!(!release.is_pre_release_of(&release));
        assert!(!version("0.1.0-rc.1").is_pre_release_of(&version("0.1.0-rc.2")));
    }
}
