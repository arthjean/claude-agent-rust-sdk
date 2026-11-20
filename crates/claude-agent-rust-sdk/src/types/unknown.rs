//! The payload of the `Unknown` variant of every string enum of the wire.

use std::fmt;

/// A wire spelling unknown to this version of the crate, kept verbatim in
/// the `Unknown` variant of a string enum such as
/// [`HookEvent`](crate::HookEvent).
///
/// Only the crate builds one, when it reads a spelling that no known
/// variant has, so an `Unknown` never stands for a known value: two values
/// of an enum are equal exactly when they are written the same on the wire.
/// To build an enum value from any text, use its `From<&str>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnknownValue(String);

impl UnknownValue {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    /// The spelling as it was read.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The spelling as an owned string.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for UnknownValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for UnknownValue {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for UnknownValue {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for UnknownValue {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}
