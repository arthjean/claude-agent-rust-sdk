//! `Debug` output that names secrets without printing them.
//!
//! Environment variables, extra CLI arguments, inline settings, and the
//! headers of a remote MCP server routinely hold API keys and tokens. The
//! `Debug` of the types that carry them prints their keys and replaces each
//! value with `<redacted>`, so a value logged with `{:?}` leaks nothing.
//! Upstream dataclasses print everything in their `repr`; this is the
//! deliberate difference recorded as `[behavior.debug-redacts-secrets]`.

use std::fmt;

use indexmap::IndexMap;
use serde_json::{Map, Value};

/// Printed in place of a secret value.
pub(crate) struct Placeholder;

impl fmt::Debug for Placeholder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// A map printed with its keys and no value.
pub(crate) struct RedactedMap<'a>(pub(crate) &'a IndexMap<String, String>);

impl fmt::Debug for RedactedMap<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.0.keys().map(|key| (key, Placeholder)))
            .finish()
    }
}

/// Extra CLI arguments printed with their names, and `None` for a flag
/// that takes no value.
pub(crate) struct RedactedArguments<'a>(pub(crate) &'a IndexMap<String, Option<String>>);

impl fmt::Debug for RedactedArguments<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(
                self.0
                    .iter()
                    .map(|(key, value)| (key, value.as_ref().map(|_| Placeholder))),
            )
            .finish()
    }
}

/// An optional map printed with its keys only.
pub(crate) fn optional_map(map: Option<&IndexMap<String, String>>) -> Option<RedactedMap<'_>> {
    map.map(RedactedMap)
}

/// The `settings` option: a path is printed, inline JSON is not, since it
/// may hold an `env` table or an API key helper.
pub(crate) struct RedactedSettings<'a>(pub(crate) Option<&'a str>);

impl fmt::Debug for RedactedSettings<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            None => f.write_str("None"),
            Some(settings) if settings.trim_start().starts_with('{') => {
                f.debug_tuple("Some").field(&Placeholder).finish()
            }
            Some(path) => f.debug_tuple("Some").field(&path).finish(),
        }
    }
}

/// A raw MCP server configuration, printed with the keys of its `env` and
/// `headers` objects and no value; every other key is printed.
pub(crate) struct RedactedRaw<'a>(pub(crate) &'a Map<String, Value>);

impl fmt::Debug for RedactedRaw<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = f.debug_map();
        for (key, value) in self.0 {
            match value {
                Value::Object(secrets) if matches!(key.as_str(), "env" | "headers") => {
                    map.entry(key, &SecretKeys(secrets));
                }
                other => {
                    map.entry(key, other);
                }
            }
        }
        map.finish()
    }
}

/// The keys of an object, each with `<redacted>`.
struct SecretKeys<'a>(&'a Map<String, Value>);

impl fmt::Debug for SecretKeys<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.0.keys().map(|key| (key, Placeholder)))
            .finish()
    }
}
