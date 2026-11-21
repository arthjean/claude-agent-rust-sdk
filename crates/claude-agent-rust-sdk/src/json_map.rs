//! Key removal on a JSON object that keeps the order of the other keys,
//! with or without the `preserve-order` feature.
//!
//! `serde_json::Map::shift_remove` exists only when `serde_json` keeps key
//! order, and `serde_json::Map::remove` then moves the last key into the
//! hole. The crate can be built either way, and another crate of the
//! program may turn `serde_json/preserve_order` on even when this crate's
//! feature is off, so removal goes through `retain`, which keeps the order
//! of the remaining keys in both representations.

use serde_json::{Map, Value};

/// Removes one key from a JSON object, keeping the order of the others.
pub(crate) trait RemoveKey {
    /// Removes `key` and returns its value, if it was present.
    fn remove_key(&mut self, key: &str) -> Option<Value>;
}

impl RemoveKey for Map<String, Value> {
    fn remove_key(&mut self, key: &str) -> Option<Value> {
        let value = self.get(key).cloned()?;
        self.retain(|name, _| name != key);
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn removing_a_key_keeps_the_others_and_returns_its_value() {
        let Value::Object(mut map) = json!({"a": 1, "b": 2, "c": 3}) else {
            unreachable!()
        };
        assert_eq!(map.remove_key("a"), Some(json!(1)));
        assert_eq!(map.remove_key("a"), None);
        let keys: Vec<&String> = map.keys().collect();
        assert_eq!(keys, ["b", "c"]);
    }
}
