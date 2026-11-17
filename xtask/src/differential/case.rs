//! Loading of the cases of `parity/cases`.
//!
//! A case is one JSON file whose name, without `.json`, is the case name.
//! Each file is checked against `parity/case.schema.json`; a case that
//! breaks it is rejected with the file and the JSON pointer of the first
//! error. A `scenario` given as a string names a file of
//! `parity/scenarios`. Every string of a case may hold `${WORK}`, replaced
//! by the temporary working directory of each side before the drivers read
//! the case.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The placeholder a case uses for the working directory of a run.
pub const WORK_VARIABLE: &str = "${WORK}";

/// One case.
#[derive(Debug, Clone)]
pub struct Case {
    /// Name of the case.
    pub name: String,
    /// The file as written, so that drivers see its keys in the author's
    /// order: the order of hook events decides the callback ids.
    pub text: String,
    /// The case, parsed, with an inline scenario.
    pub value: Value,
    /// Variables of the driver process.
    pub process_env: Vec<(String, String)>,
    /// Groups of frame labels compared as sets.
    pub unordered: Vec<Vec<String>>,
}

impl Case {
    /// The text of the case with `${WORK}` replaced by `work`, escaped for
    /// a JSON string.
    pub fn resolved_text(&self, work: &str) -> String {
        let escaped = Value::String(work.to_owned()).to_string();
        let escaped = escaped.trim_matches('"');
        self.text.replace(WORK_VARIABLE, escaped)
    }

    /// The scenario with `${WORK}` replaced in every string.
    pub fn resolved_scenario(&self, work: &str) -> Value {
        replace(&self.value["scenario"], work)
    }

    /// The option names the case sets.
    pub fn option_names(&self) -> Vec<String> {
        self.value
            .get("options")
            .and_then(Value::as_object)
            .map(|options| options.keys().cloned().collect())
            .unwrap_or_default()
    }
}

fn replace(value: &Value, work: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace(WORK_VARIABLE, work)),
        Value::Array(items) => items.iter().map(|item| replace(item, work)).collect(),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| (key.clone(), replace(value, work)))
            .collect::<serde_json::Map<String, Value>>()
            .into(),
        other => other.clone(),
    }
}

fn read_text(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn parse_json(path: &Path, text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|error| format!("{}: invalid JSON: {error}", path.display()))
}

fn read_json(path: &Path) -> Result<Value, String> {
    parse_json(path, &read_text(path)?)
}

/// The case files of a directory, sorted by name.
pub fn files(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let listing_error = |error| format!("cannot list {}: {error}", directory.display());
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(listing_error)? {
        let path = entry.map_err(listing_error)?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Loads every case of `cases`, checked against `schema`, with scenarios
/// named by string read from `scenarios`.
pub fn load_all(cases: &Path, scenarios: &Path, schema: &Path) -> Result<Vec<Case>, String> {
    let schema_value = read_json(schema)?;
    let validator = jsonschema::validator_for(&schema_value)
        .map_err(|error| format!("{}: invalid schema: {error}", schema.display()))?;
    let mut loaded = Vec::new();
    for path in files(cases)? {
        let text = read_text(&path)?;
        let value = parse_json(&path, &text)?;
        if let Err(error) = validator.validate(&value) {
            let pointer = error.instance_path().to_string();
            let pointer = if pointer.is_empty() {
                "/".to_owned()
            } else {
                pointer
            };
            return Err(format!(
                "{}: does not match {} at {pointer}: {error}",
                path.display(),
                schema.display()
            ));
        }
        loaded.push(load_one(&path, text, value, scenarios)?);
    }
    Ok(loaded)
}

fn load_one(path: &Path, text: String, mut value: Value, scenarios: &Path) -> Result<Case, String> {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = value["name"].as_str().unwrap_or_default().to_owned();
    if name != stem {
        return Err(format!(
            "{}: the case is named `{name}`; it must be named after its file, `{stem}`",
            path.display()
        ));
    }
    if let Some(scenario) = value["scenario"].as_str() {
        let scenario_path = scenarios.join(format!("{scenario}.json"));
        value["scenario"] =
            read_json(&scenario_path).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let process_env = value
        .get("process_env")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().to_owned()))
        .collect();
    let unordered = value
        .get("unordered")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|group| {
            group
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .collect();
    Ok(Case {
        name,
        text,
        value,
        process_env,
        unordered,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::load_all;

    fn schema() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("parity")
            .join("case.schema.json")
    }

    fn write_case(directory: &Path, name: &str, case: &serde_json::Value) {
        std::fs::write(directory.join(format!("{name}.json")), case.to_string()).unwrap();
    }

    #[test]
    fn a_valid_case_loads_with_its_named_scenario_and_placeholders() {
        let directory = tempfile::tempdir().unwrap();
        let cases = directory.path().join("cases");
        let scenarios = directory.path().join("scenarios");
        std::fs::create_dir_all(&cases).unwrap();
        std::fs::create_dir_all(&scenarios).unwrap();
        std::fs::write(scenarios.join("one-shot.json"), r#"{"steps": []}"#).unwrap();
        std::fs::write(cases.join("coverage.toml"), "").unwrap();
        write_case(
            &cases,
            "with-cwd",
            &json!({
                "name": "with-cwd",
                "scenario": "one-shot",
                "options": {"cwd": "${WORK}/project"},
                "prompt": "hi",
                "process_env": {"CLAUDECODE": "1"},
                "unordered": [["a", "b"]],
            }),
        );
        let loaded = load_all(&cases, &scenarios, &schema()).unwrap();
        assert_eq!(loaded.len(), 1);
        let case = &loaded[0];
        assert_eq!(case.value["scenario"], json!({"steps": []}));
        assert_eq!(
            case.process_env,
            [("CLAUDECODE".to_owned(), "1".to_owned())]
        );
        assert_eq!(case.unordered, [vec!["a".to_owned(), "b".to_owned()]]);
        assert_eq!(case.option_names(), ["cwd"]);
        let resolved: serde_json::Value =
            serde_json::from_str(&case.resolved_text("C:\\w")).unwrap();
        assert_eq!(resolved["options"]["cwd"], "C:\\w/project");
        assert_eq!(resolved["scenario"], "one-shot");
        assert_eq!(case.resolved_scenario("/w"), json!({"steps": []}));
    }

    #[test]
    fn a_case_that_breaks_the_schema_names_the_file_and_the_pointer() {
        let directory = tempfile::tempdir().unwrap();
        let invalid = [
            (json!({"name": "a", "scenario": {}, "prompt": 3}), "/prompt"),
            (
                json!({"name": "a", "scenario": {}, "prompt": "x", "steps": [{"interrupt": true}]}),
                "/",
            ),
            (
                json!({"name": "a", "scenario": {}, "steps": [{"dance": true}]}),
                "/steps/0",
            ),
            (
                json!({"name": "a", "scenario": {}, "prompt": "x", "colour": 1}),
                "/",
            ),
        ];
        for (case, pointer) in invalid {
            write_case(directory.path(), "a", &case);
            let error = load_all(directory.path(), directory.path(), &schema()).unwrap_err();
            assert!(error.contains("a.json"), "{error}");
            assert!(
                error.contains(&format!("at {pointer}:")),
                "{pointer}: {error}"
            );
        }
    }

    #[test]
    fn a_case_must_be_named_after_its_file() {
        let directory = tempfile::tempdir().unwrap();
        write_case(
            directory.path(),
            "a",
            &json!({"name": "b", "scenario": {}, "prompt": "x"}),
        );
        let error = load_all(directory.path(), directory.path(), &schema()).unwrap_err();
        assert!(error.contains("named after its file"), "{error}");
    }
}
