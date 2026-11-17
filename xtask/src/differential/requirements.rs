//! `cargo xtask parity-diff --check-requirements`: the locked Python
//! dependencies of the differential job agree with upstream.
//!
//! `parity/requirements.txt` pins every package the Python driver needs,
//! each with its hashes, for `pip install --require-hashes`. This check
//! reads the runtime dependencies of the submodule's `pyproject.toml` and
//! fails, naming the package, when one of them is not pinned, when its pin
//! is outside the range upstream allows, or when a pinned package carries
//! no hash. Environment markers are evaluated for the Python version of the
//! job, [`JOB_PYTHON`]. `mcp` must also stay on the 1.x line the job covers.

use std::collections::BTreeMap;
use std::path::Path;

/// Python version of the differential job.
pub const JOB_PYTHON: (u64, u64) = (3, 13);
const HELD_MAJOR_LINES: [(&str, u64); 1] = [("mcp", 1)];

fn normalize(name: &str) -> String {
    name.trim().to_lowercase().replace(['_', '.'], "-")
}

fn release(version: &str) -> Vec<u64> {
    version
        .trim()
        .split('.')
        .map_while(|part| {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .collect()
}

fn compare(left: &[u64], right: &[u64]) -> std::cmp::Ordering {
    let length = left.len().max(right.len());
    let pad = |parts: &[u64]| {
        let mut padded = parts.to_vec();
        padded.resize(length, 0);
        padded
    };
    pad(left).cmp(&pad(right))
}

fn satisfies(version: &str, specifier: &str) -> Result<bool, String> {
    let specifier = specifier.trim();
    let operator_length = specifier
        .find(|character: char| character.is_ascii_digit())
        .ok_or_else(|| format!("cannot read the specifier `{specifier}`"))?;
    let (operator, bound) = specifier.split_at(operator_length);
    let ordering = compare(&release(version), &release(bound));
    use std::cmp::Ordering::{Equal, Greater, Less};
    Ok(match operator.trim() {
        ">=" => ordering != Less,
        ">" => ordering == Greater,
        "<=" => ordering != Greater,
        "<" => ordering == Less,
        "==" => ordering == Equal,
        "!=" => ordering != Equal,
        other => return Err(format!("unsupported operator `{other}` in `{specifier}`")),
    })
}

fn marker_holds(marker: &str) -> Result<bool, String> {
    let marker = marker.trim();
    let rest = marker
        .strip_prefix("python_version")
        .ok_or_else(|| format!("unsupported environment marker `{marker}`"))?;
    let bound = rest
        .trim_start_matches(['<', '>', '=', '!', ' '])
        .trim()
        .trim_matches(['\'', '"']);
    let operator =
        rest[..rest.len() - rest.trim_start_matches(['<', '>', '=', '!', ' ']).len()].trim();
    satisfies(
        &format!("{}.{}", JOB_PYTHON.0, JOB_PYTHON.1),
        &format!("{operator}{bound}"),
    )
}

struct Requirement {
    name: String,
    specifiers: Vec<String>,
    marker: Option<String>,
}

fn requirement(text: &str) -> Requirement {
    let (requirement, marker) = match text.split_once(';') {
        Some((requirement, marker)) => (requirement, Some(marker.trim().to_owned())),
        None => (text, None),
    };
    let start = requirement
        .find(['<', '>', '=', '!', '~'])
        .unwrap_or(requirement.len());
    let (name, specifiers) = requirement.split_at(start);
    let name = name.split('[').next().unwrap_or(name);
    Requirement {
        name: normalize(name),
        specifiers: specifiers
            .split(',')
            .map(str::trim)
            .filter(|specifier| !specifier.is_empty())
            .map(str::to_owned)
            .collect(),
        marker,
    }
}

/// A pinned package: its version and whether a hash follows it.
#[derive(Debug, PartialEq)]
pub struct Pin {
    /// The pinned version.
    pub version: String,
    /// Whether the pin carries at least one `--hash`.
    pub hashed: bool,
}

/// Reads the pins of a requirements file written with hashes.
pub fn pins(text: &str) -> Result<BTreeMap<String, Pin>, String> {
    let mut pins = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let content = line.trim_end_matches('\\').trim();
        if content.starts_with("--hash=") {
            let name = current
                .as_ref()
                .ok_or_else(|| format!("a hash comes before any package: `{line}`"))?;
            if let Some(pin) = pins.get_mut(name) {
                let pin: &mut Pin = pin;
                pin.hashed = true;
            }
            continue;
        }
        let pinned = content
            .split(';')
            .next()
            .unwrap_or(content)
            .split_whitespace()
            .next()
            .unwrap_or_default();
        let (name, version) = pinned
            .split_once("==")
            .ok_or_else(|| format!("`{line}` is not pinned with =="))?;
        let name = normalize(name);
        let hashed = content.contains("--hash=");
        pins.insert(
            name.clone(),
            Pin {
                version: version.trim().to_owned(),
                hashed,
            },
        );
        current = Some(name);
    }
    Ok(pins)
}

/// The upstream runtime dependencies, as written in `pyproject.toml`.
pub fn dependencies(pyproject: &str) -> Result<Vec<String>, String> {
    let value: toml::Table = toml::from_str(pyproject).map_err(|error| error.to_string())?;
    value
        .get("project")
        .and_then(|project| project.get("dependencies"))
        .and_then(toml::Value::as_array)
        .map(|dependencies| {
            dependencies
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .ok_or_else(|| "pyproject.toml has no [project].dependencies".to_owned())
}

/// Problems between the upstream dependencies and the pins.
pub fn check(dependencies: &[String], pins: &BTreeMap<String, Pin>) -> Result<Vec<String>, String> {
    let mut problems = Vec::new();
    for dependency in dependencies {
        let requirement = requirement(dependency);
        if let Some(marker) = &requirement.marker
            && !marker_holds(marker)?
        {
            continue;
        }
        let Some(pin) = pins.get(&requirement.name) else {
            problems.push(format!(
                "{}: required by upstream as `{dependency}` but not pinned",
                requirement.name
            ));
            continue;
        };
        for specifier in &requirement.specifiers {
            if !satisfies(&pin.version, specifier)? {
                problems.push(format!(
                    "{}: pinned at {}, outside the range upstream allows (`{dependency}`)",
                    requirement.name, pin.version
                ));
                break;
            }
        }
    }
    for (name, major) in HELD_MAJOR_LINES {
        if let Some(pin) = pins.get(name)
            && release(&pin.version).first() != Some(&major)
        {
            problems.push(format!(
                "{name}: pinned at {}, but the differential job covers the {major}.x line only",
                pin.version
            ));
        }
    }
    for (name, pin) in pins {
        if !pin.hashed {
            problems.push(format!("{name}: pinned at {} without a hash", pin.version));
        }
    }
    Ok(problems)
}

/// Runs the check of `requirements` against the submodule's `pyproject.toml`.
pub fn run(upstream: &Path, requirements: &Path) -> Result<bool, String> {
    let read = |path: &Path| {
        std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))
    };
    let dependencies = dependencies(&read(&upstream.join("pyproject.toml"))?)?;
    let pins = pins(&read(requirements)?)
        .map_err(|error| format!("{}: {error}", requirements.display()))?;
    let problems = check(&dependencies, &pins)?;
    for problem in &problems {
        println!("error: {problem}");
    }
    if problems.is_empty() {
        println!(
            "{} upstream dependencies agree with the {} pins of {}",
            dependencies.len(),
            pins.len(),
            requirements.display()
        );
    }
    Ok(problems.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{check, dependencies, pins};

    const PYPROJECT: &str = r#"
[project]
dependencies = [
    "anyio>=4.0.0",
    "typing_extensions>=4.0.0; python_version<'3.11'",
    "mcp>=1.23.0,<3.0.0",
    "jsonschema>=4.20.0",
]
"#;

    fn locked(mcp: &str, anyio_hash: bool) -> String {
        let anyio_hash = if anyio_hash {
            " \\\n    --hash=sha256:aa"
        } else {
            ""
        };
        format!(
            "# header\nanyio==4.16.0{anyio_hash}\nmcp=={mcp} \\\n    --hash=sha256:bb \\\n    --hash=sha256:cc\n\
             jsonschema==4.26.0 \\\n    --hash=sha256:dd\n    # via mcp\n"
        )
    }

    #[test]
    fn pins_within_the_upstream_ranges_pass() {
        let pins = pins(&locked("1.30.0", true)).unwrap();
        assert_eq!(pins.len(), 3);
        assert!(pins.values().all(|pin| pin.hashed));
        let problems = check(&dependencies(PYPROJECT).unwrap(), &pins).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn a_pin_outside_the_range_fails_naming_the_package() {
        let pins = pins(&locked("1.20.0", true)).unwrap();
        let problems = check(&dependencies(PYPROJECT).unwrap(), &pins).unwrap();
        assert_eq!(
            problems,
            ["mcp: pinned at 1.20.0, outside the range upstream allows (`mcp>=1.23.0,<3.0.0`)"]
        );
    }

    #[test]
    fn a_missing_pin_a_second_major_line_and_a_missing_hash_fail() {
        let mut pins = pins(&locked("2.1.0", false)).unwrap();
        pins.remove("jsonschema");
        let problems = check(&dependencies(PYPROJECT).unwrap(), &pins).unwrap();
        assert_eq!(
            problems,
            [
                "jsonschema: required by upstream as `jsonschema>=4.20.0` but not pinned",
                "mcp: pinned at 2.1.0, but the differential job covers the 1.x line only",
                "anyio: pinned at 4.16.0 without a hash",
            ]
        );
    }
}
