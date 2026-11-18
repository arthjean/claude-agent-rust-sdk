//! `CHANGELOG.md`: its format, the section of one version, and the edits
//! the release commands make to it.
//!
//! ```markdown
//! # Changelog
//!
//! ## Unreleased
//!
//! ### Added
//!
//! - What the next release adds.
//!
//! ## 0.1.1 - 2026-11-20
//!
//! Upstream: Python SDK 0.2.183 to 0.2.185, CLI 2.1.330
//!
//! ### Fixed
//!
//! - What it fixed.
//! ```
//!
//! Released sections are titled `## <version> - <YYYY-MM-DD>`, newest
//! first, and open with their `Upstream:` line. Entries are grouped under
//! [`GROUPS`], in that order, and only groups with entries appear.
//!
//! The section of a release that follows pre-releases (`0.1.0` after
//! `0.1.0-rc.1` and `0.1.0-rc.2`) gathers their entries, oldest first,
//! before its own, because its reader upgrades from the previous release
//! and never read the pre-release sections. Those sections stay as they
//! were published.

use std::fmt::Write as _;

use super::version::Version;

/// The title of the file.
pub const TITLE: &str = "# Changelog";
/// The title of the section that collects the next release.
pub const UNRELEASED: &str = "## Unreleased";
/// The entry groups, in the order a section lists them.
pub const GROUPS: [&str; 4] = ["Breaking changes", "Added", "Changed", "Fixed"];
/// How every entry `upstream-sync` writes begins, so a release can tell
/// them from hand-written entries.
pub const SYNC_ENTRY_PREFIX: &str = "- Upstream moved to ";

/// One `## ` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The title line without `## `.
    pub title: String,
    /// Everything between the title line and the next section.
    pub body: String,
}

/// The parsed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changelog {
    /// Everything before the first section.
    pub preamble: String,
    /// The sections, in file order.
    pub sections: Vec<Section>,
}

/// A released section, read from its title and its `Upstream:` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The version of the title.
    pub version: Version,
    /// The oldest upstream SDK version its `Upstream:` line names.
    pub first_upstream: String,
    /// The newest upstream SDK version its `Upstream:` line names.
    pub last_upstream: String,
}

impl Changelog {
    /// Splits `text` into its preamble and its sections.
    pub fn parse(text: &str) -> Self {
        let text = text.replace("\r\n", "\n");
        let mut preamble = String::new();
        let mut sections: Vec<Section> = Vec::new();
        for line in text.split_inclusive('\n') {
            if let Some(title) = line.strip_prefix("## ") {
                sections.push(Section {
                    title: title.trim_end().to_owned(),
                    body: String::new(),
                });
            } else if let Some(section) = sections.last_mut() {
                section.body.push_str(line);
            } else {
                preamble.push_str(line);
            }
        }
        Self { preamble, sections }
    }

    /// Writes the file back.
    pub fn render(&self) -> String {
        let mut text = self.preamble.clone();
        for section in &self.sections {
            let _ = writeln!(text, "## {}", section.title);
            text.push_str(&section.body);
        }
        format!("{}\n", text.trim_end())
    }

    /// The format problems of the file, one message each.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.preamble.trim() != TITLE {
            problems.push(format!(
                "CHANGELOG.md must open with `{TITLE}` and nothing else before its first section"
            ));
        }
        if self.sections.first().map(|section| section.title.as_str()) != Some("Unreleased") {
            problems.push(format!(
                "the first section of CHANGELOG.md must be `{UNRELEASED}`"
            ));
        }
        let mut previous: Option<Version> = None;
        for section in self.sections.iter().skip(1) {
            let Some(version) = released_version(&section.title) else {
                problems.push(format!(
                    "`## {}` is not titled `## <version> - <YYYY-MM-DD>`",
                    section.title
                ));
                continue;
            };
            if upstream_line(&section.body).is_none() {
                problems.push(format!(
                    "`## {}` does not start with `Upstream: Python SDK <version>[ to <version>], CLI <version>`",
                    section.title
                ));
            }
            for heading in section
                .body
                .lines()
                .filter_map(|line| line.strip_prefix("### "))
            {
                if !GROUPS.contains(&heading.trim()) {
                    problems.push(format!(
                        "`## {}` has the group `### {heading}`, not one of {}",
                        section.title,
                        GROUPS.join(", ")
                    ));
                }
            }
            if let Some(previous) = &previous
                && version >= *previous
            {
                problems.push(format!(
                    "`## {}` must be older than the section above it ({previous})",
                    section.title
                ));
            }
            previous = Some(version);
        }
        problems
    }

    /// The released sections that parse, newest first.
    pub fn releases(&self) -> Vec<Release> {
        self.sections
            .iter()
            .filter_map(|section| {
                let version = released_version(&section.title)?;
                let (first, last) = upstream_line(&section.body)?;
                Some(Release {
                    version,
                    first_upstream: first,
                    last_upstream: last,
                })
            })
            .collect()
    }

    /// The body of the released section of `version`.
    pub fn section_of(&self, version: &str) -> Option<&str> {
        self.sections
            .iter()
            .find(|section| {
                released_version(&section.title).is_some_and(|found| found.to_string() == version)
            })
            .map(|section| section.body.as_str())
    }

    fn unreleased_mut(&mut self) -> Result<&mut Section, String> {
        self.sections
            .first_mut()
            .filter(|section| section.title == "Unreleased")
            .ok_or_else(|| format!("CHANGELOG.md has no `{UNRELEASED}` section first"))
    }

    /// The body of `## Unreleased`.
    #[cfg(test)]
    pub fn unreleased(&self) -> Option<&str> {
        self.sections
            .first()
            .filter(|section| section.title == "Unreleased")
            .map(|section| section.body.as_str())
    }

    /// Turns `## Unreleased` into the section of `version`, opened by
    /// `upstream`, and inserts a new empty `## Unreleased` above it.
    /// `drafted` is added to the entries when the section holds no entry
    /// written by hand. For a release, the entries of its pre-release
    /// sections come first, oldest first, each entry once.
    pub fn release(
        &mut self,
        version: &Version,
        date: &str,
        upstream: &str,
        drafted: &Entries,
    ) -> Result<(), String> {
        let mut gathered: Vec<String> = self
            .sections
            .iter()
            .rev()
            .filter(|section| {
                released_version(&section.title)
                    .is_some_and(|found| found.is_pre_release_of(version))
            })
            .map(|section| section.body.clone())
            .collect();
        let unreleased = self.unreleased_mut()?;
        let mut body = unreleased.body.trim().to_owned();
        if !has_hand_written_entry(&body) {
            for (group, entries) in GROUPS.iter().zip(&drafted.groups) {
                for entry in entries {
                    body = with_entry(&body, group, entry);
                }
            }
        }
        if !gathered.is_empty() {
            gathered.push(body);
            body = merged(&gathered);
        }
        let body = body.trim();
        unreleased.title = format!("{version} - {date}");
        unreleased.body = if body.is_empty() {
            format!("\n{upstream}\n\n")
        } else {
            format!("\n{upstream}\n\n{body}\n\n")
        };
        self.sections.insert(
            0,
            Section {
                title: "Unreleased".to_owned(),
                body: "\n".to_owned(),
            },
        );
        Ok(())
    }

    /// Adds `entry` (one Markdown line starting with `- `) under `group` of
    /// `## Unreleased`.
    pub fn add_unreleased_entry(&mut self, group: &str, entry: &str) -> Result<(), String> {
        let unreleased = self.unreleased_mut()?;
        let body = with_entry(unreleased.body.trim(), group, entry);
        unreleased.body = format!("\n{}\n\n", body.trim());
        Ok(())
    }
}

/// The entries of a section body in file order, each with its group
/// (`None` before the first `### ` heading). An entry is a line and the
/// indented lines that continue it; the `Upstream:` line is left out.
fn entries_of(body: &str) -> Vec<(Option<String>, String)> {
    let mut entries: Vec<(Option<String>, String)> = Vec::new();
    let mut group: Option<String> = None;
    let mut continues = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continues = false;
        } else if let Some(heading) = trimmed.strip_prefix("### ") {
            group = Some(heading.trim().to_owned());
            continues = false;
        } else if trimmed.starts_with("Upstream: ") && entries.is_empty() && group.is_none() {
            continues = false;
        } else if continues && line.starts_with(char::is_whitespace) {
            if let Some((_, entry)) = entries.last_mut() {
                entry.push('\n');
                entry.push_str(line.trim_end());
            }
        } else {
            entries.push((group.clone(), line.trim_end().to_owned()));
            continues = true;
        }
    }
    entries
}

/// One body holding the entries of `bodies`, in their order and each
/// once, under their groups in the order of [`GROUPS`]. Text outside any
/// group comes first.
fn merged(bodies: &[String]) -> String {
    let mut loose: Vec<String> = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut body = String::new();
    for (group, entry) in bodies.iter().flat_map(|body| entries_of(body)) {
        match group {
            None => {
                if !loose.contains(&entry) {
                    loose.push(entry);
                }
            }
            Some(group) => {
                let key = (group, entry);
                if !seen.contains(&key) {
                    body = with_entry(&body, &key.0, &key.1);
                    seen.push(key);
                }
            }
        }
    }
    if loose.is_empty() {
        body
    } else {
        format!("{}\n\n{body}", loose.join("\n"))
    }
}

/// Whether a section body has content beyond its `Upstream:` line.
pub fn has_content(body: &str) -> bool {
    body.lines().map(str::trim).any(|line| {
        !line.is_empty() && !line.starts_with("Upstream: ") && !line.starts_with("### ")
    })
}

fn has_hand_written_entry(body: &str) -> bool {
    body.lines().map(str::trim).any(|line| {
        !line.is_empty() && !line.starts_with("### ") && !line.starts_with(SYNC_ENTRY_PREFIX)
    })
}

/// The version of a released section title, `<version> - <YYYY-MM-DD>`.
pub fn released_version(title: &str) -> Option<Version> {
    let (version, date) = title.split_once(" - ")?;
    let bytes = date.as_bytes();
    let date_ok = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit());
    if !date_ok {
        return None;
    }
    Version::parse(version).ok()
}

/// The first and last upstream SDK versions of the `Upstream:` line that
/// opens `body`.
pub fn upstream_line(body: &str) -> Option<(String, String)> {
    let line = body.lines().find(|line| !line.trim().is_empty())?;
    let rest = line.trim().strip_prefix("Upstream: Python SDK ")?;
    let (range, cli) = rest.split_once(", CLI ")?;
    Version::parse(cli).ok()?;
    let (first, last) = range.split_once(" to ").unwrap_or((range, range));
    Version::parse(first).ok()?;
    Version::parse(last).ok()?;
    Some((first.to_owned(), last.to_owned()))
}

/// The `Upstream:` line of a release.
pub fn upstream_text(first: &str, last: &str, cli: &str) -> String {
    if first == last {
        format!("Upstream: Python SDK {last}, CLI {cli}")
    } else {
        format!("Upstream: Python SDK {first} to {last}, CLI {cli}")
    }
}

/// Inserts `entry` at the end of the `### group` of `body`, creating the
/// group in the order of [`GROUPS`] when it is missing.
fn with_entry(body: &str, group: &str, entry: &str) -> String {
    let mut lines: Vec<String> = body.lines().map(str::to_owned).collect();
    let heading = format!("### {group}");
    let rank = GROUPS.iter().position(|known| *known == group);
    if let Some(start) = lines.iter().position(|line| line.trim() == heading) {
        let end = lines
            .iter()
            .enumerate()
            .skip(start + 1)
            .find(|(_, line)| line.starts_with("### "))
            .map_or(lines.len(), |(index, _)| index);
        let mut last_entry = start;
        for (index, line) in lines.iter().enumerate().take(end).skip(start + 1) {
            if !line.trim().is_empty() {
                last_entry = index;
            }
        }
        if last_entry == start {
            lines.insert(start + 1, String::new());
            lines.insert(start + 2, entry.to_owned());
        } else {
            lines.insert(last_entry + 1, entry.to_owned());
        }
        return lines.join("\n");
    }
    let later = lines.iter().position(|line| {
        line.strip_prefix("### ").is_some_and(|name| {
            let position = GROUPS.iter().position(|known| *known == name.trim());
            matches!((position, rank), (Some(position), Some(rank)) if position > rank)
        })
    });
    let block = [heading, String::new(), entry.to_owned()];
    match later {
        Some(index) => {
            let mut inserted: Vec<String> = block.to_vec();
            inserted.push(String::new());
            lines.splice(index..index, inserted);
        }
        None => {
            if lines.iter().any(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.extend(block);
        }
    }
    lines.join("\n")
}

/// Entries drafted from commits, one list per group of [`GROUPS`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entries {
    /// The entries of each group, in the order of [`GROUPS`].
    pub groups: [Vec<String>; 4],
}

/// A conventional commit subject: `type(scope)!: description`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conventional {
    /// The type, lowercase.
    pub kind: String,
    /// The description after `: `.
    pub description: String,
    /// Whether the subject has `!` or the message a `BREAKING CHANGE:`
    /// footer.
    pub breaking: bool,
}

/// Reads one commit message, `None` when its subject is not conventional.
pub fn conventional(message: &str) -> Option<Conventional> {
    let subject = message.lines().next()?.trim();
    let (head, description) = subject.split_once(": ")?;
    let (head, bang) = match head.strip_suffix('!') {
        Some(head) => (head, true),
        None => (head, false),
    };
    let kind = match head.split_once('(') {
        Some((kind, scope)) => {
            if !scope.ends_with(')') || scope.len() < 2 {
                return None;
            }
            kind
        }
        None => head,
    };
    if kind.is_empty() || !kind.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return None;
    }
    let footer = message
        .lines()
        .skip(1)
        .any(|line| line.starts_with("BREAKING CHANGE:") || line.starts_with("BREAKING-CHANGE:"));
    Some(Conventional {
        kind: kind.to_owned(),
        description: description.trim().to_owned(),
        breaking: bang || footer,
    })
}

/// Drafts entries from commit messages, oldest first: a breaking commit
/// under Breaking changes, `feat` under Added, `perf` and `refactor` under
/// Changed, `fix` under Fixed, every other type left out.
pub fn draft(messages: &[String]) -> Entries {
    let mut entries = Entries::default();
    for commit in messages.iter().filter_map(|message| conventional(message)) {
        let group = if commit.breaking {
            Some(0)
        } else {
            match commit.kind.as_str() {
                "feat" => Some(1),
                "perf" | "refactor" => Some(2),
                "fix" => Some(3),
                _ => None,
            }
        };
        if let Some(group) = group {
            entries.groups[group].push(format!("- {}", commit.description));
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Changelog\n\n## Unreleased\n\n### Added\n\n- New thing.\n\n## 0.1.1 - 2026-11-20\n\nUpstream: Python SDK 0.2.183 to 0.2.185, CLI 2.1.330\n\n### Fixed\n\n- A fix.\n\n## 0.1.0 - 2026-11-19\n\nUpstream: Python SDK 0.2.182, CLI 2.1.325\n\n### Added\n\n- First.\n";

    #[test]
    fn the_file_round_trips_and_a_section_body_is_found_by_version() {
        let changelog = Changelog::parse(SAMPLE);
        assert_eq!(changelog.render(), SAMPLE);
        assert!(
            changelog.problems().is_empty(),
            "{:?}",
            changelog.problems()
        );
        let body = changelog.section_of("0.1.1").unwrap();
        assert!(body.contains("- A fix."));
        assert!(!body.contains("First."));
        assert!(changelog.section_of("0.1.2").is_none());
        let releases = changelog.releases();
        assert_eq!(releases[0].last_upstream, "0.2.185");
        assert_eq!(releases[1].last_upstream, "0.2.182");
    }

    #[test]
    fn format_problems_name_the_section() {
        let broken = "# Changelog\n\n## Unreleased\n\n## 0.1.0 - 2026-11-19\n\nUpstream: Python SDK 0.2.182, CLI 2.1.325\n\n## 0.1.1 - 2026-11-20\n\n- No upstream line.\n\n## 0.0.9 (2026-11-01)\n\n### Removed\n";
        let problems = Changelog::parse(broken).problems();
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("`## 0.1.1 - 2026-11-20` does not start"))
        );
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("`## 0.1.1 - 2026-11-20` must be older"))
        );
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("`## 0.0.9 (2026-11-01)` is not titled"))
        );
        let missing = Changelog::parse("# Changelog\n\n## 0.1.0 - 2026-11-19\n").problems();
        assert!(missing.iter().any(|problem| problem.contains("Unreleased")));
    }

    #[test]
    fn a_release_keeps_hand_written_entries_and_opens_a_new_unreleased_section() {
        let mut changelog = Changelog::parse(SAMPLE);
        let drafted = draft(&["feat: drafted".to_owned()]);
        changelog
            .release(
                &Version::parse("0.2.0").unwrap(),
                "2026-11-21",
                "Upstream: Python SDK 0.2.186, CLI 2.1.331",
                &drafted,
            )
            .unwrap();
        let text = changelog.render();
        assert!(text.starts_with(
            "# Changelog\n\n## Unreleased\n\n## 0.2.0 - 2026-11-21\n\nUpstream: Python SDK 0.2.186, CLI 2.1.331\n\n### Added\n\n- New thing.\n\n## 0.1.1"
        ), "{text}");
        assert!(!text.contains("drafted"));
        assert!(Changelog::parse(&text).problems().is_empty());
    }

    const CANDIDATES: &str = "# Changelog\n\n## Unreleased\n\n### Fixed\n\n- A late fix.\n\n## 0.2.0-rc.2 - 2026-11-21\n\nUpstream: Python SDK 0.2.186, CLI 2.1.331\n\n### Changed\n\n- Second candidate.\n\n### Fixed\n\n- A candidate fix.\n\n## 0.2.0-rc.1 - 2026-11-20\n\nUpstream: Python SDK 0.2.184 to 0.2.185, CLI 2.1.330\n\nA paragraph about the candidates.\n\n### Added\n\n- First candidate,\n  on two lines.\n\n### Fixed\n\n- A candidate fix.\n\n## 0.1.0 - 2026-11-19\n\nUpstream: Python SDK 0.2.182, CLI 2.1.325\n\n### Added\n\n- First.\n";

    #[test]
    fn a_release_gathers_the_entries_of_its_pre_releases_oldest_first_and_once() {
        let mut changelog = Changelog::parse(CANDIDATES);
        changelog
            .release(
                &Version::parse("0.2.0").unwrap(),
                "2026-11-22",
                "Upstream: Python SDK 0.2.184 to 0.2.186, CLI 2.1.331",
                &draft(&["feat: drafted".to_owned()]),
            )
            .unwrap();
        assert_eq!(
            changelog.section_of("0.2.0").unwrap(),
            "\nUpstream: Python SDK 0.2.184 to 0.2.186, CLI 2.1.331\n\nA paragraph about the candidates.\n\n### Added\n\n- First candidate,\n  on two lines.\n\n### Changed\n\n- Second candidate.\n\n### Fixed\n\n- A candidate fix.\n- A late fix.\n\n"
        );
        let text = changelog.render();
        assert!(
            !text.contains("drafted"),
            "hand-written entries win: {text}"
        );
        assert!(
            text.contains("## 0.2.0-rc.2 - 2026-11-21\n\nUpstream: Python SDK 0.2.186, CLI 2.1.331\n\n### Changed\n\n- Second candidate.\n\n### Fixed\n\n- A candidate fix.\n\n## 0.2.0-rc.1"),
            "a pre-release section stays as published: {text}"
        );
        assert!(
            text.contains("## 0.1.0 - 2026-11-19\n\nUpstream: Python SDK 0.2.182, CLI 2.1.325\n\n### Added\n\n- First.\n")
        );
        assert!(Changelog::parse(&text).problems().is_empty());
        let releases = changelog.releases();
        assert_eq!(releases[0].first_upstream, "0.2.184");
        assert_eq!(releases[2].first_upstream, "0.2.184");
        assert_eq!(releases[2].last_upstream, "0.2.185");
    }

    #[test]
    fn a_pre_release_or_another_release_gathers_nothing() {
        for version in ["0.2.0-rc.3", "0.2.1"] {
            let mut changelog = Changelog::parse(CANDIDATES);
            changelog
                .release(
                    &Version::parse(version).unwrap(),
                    "2026-11-22",
                    "Upstream: Python SDK 0.2.186, CLI 2.1.331",
                    &Entries::default(),
                )
                .unwrap();
            assert_eq!(
                changelog.section_of(version).unwrap(),
                "\nUpstream: Python SDK 0.2.186, CLI 2.1.331\n\n### Fixed\n\n- A late fix.\n\n",
                "{version}"
            );
        }
    }

    #[test]
    fn an_empty_unreleased_section_takes_the_drafted_entries_in_group_order() {
        let mut changelog = Changelog::parse(
            "# Changelog\n\n## Unreleased\n\n### Changed\n\n- Upstream moved to Python SDK 0.2.183 and CLI 2.1.326.\n",
        );
        let drafted = draft(&[
            "fix(cli): a fix".to_owned(),
            "feat: a feature".to_owned(),
            "docs: left out".to_owned(),
            "refactor!: a break".to_owned(),
            "perf: faster".to_owned(),
            "chore: left out".to_owned(),
            "feat: footer break\n\nBREAKING CHANGE: the API moved".to_owned(),
            "Merge branch main".to_owned(),
        ]);
        changelog
            .release(
                &Version::parse("0.2.0").unwrap(),
                "2026-11-21",
                "Upstream: Python SDK 0.2.183, CLI 2.1.326",
                &drafted,
            )
            .unwrap();
        let body = changelog.section_of("0.2.0").unwrap();
        assert_eq!(
            body,
            "\nUpstream: Python SDK 0.2.183, CLI 2.1.326\n\n### Breaking changes\n\n- a break\n- footer break\n\n### Added\n\n- a feature\n\n### Changed\n\n- Upstream moved to Python SDK 0.2.183 and CLI 2.1.326.\n- faster\n\n### Fixed\n\n- a fix\n\n"
        );
    }

    #[test]
    fn an_unreleased_entry_lands_under_its_group() {
        let mut changelog =
            Changelog::parse("# Changelog\n\n## Unreleased\n\n### Fixed\n\n- A fix.\n");
        changelog
            .add_unreleased_entry("Changed", "- Upstream moved to X.")
            .unwrap();
        assert_eq!(
            changelog.unreleased().unwrap(),
            "\n### Changed\n\n- Upstream moved to X.\n\n### Fixed\n\n- A fix.\n\n"
        );
        changelog
            .add_unreleased_entry("Changed", "- Another.")
            .unwrap();
        assert!(
            changelog
                .unreleased()
                .unwrap()
                .contains("- Upstream moved to X.\n- Another.\n")
        );
        let mut empty = Changelog::parse("# Changelog\n\n## Unreleased\n");
        empty.add_unreleased_entry("Added", "- One.").unwrap();
        assert_eq!(empty.unreleased().unwrap(), "\n### Added\n\n- One.\n\n");
    }

    #[test]
    fn commit_subjects_parse_as_conventional_or_not() {
        let breaking = conventional("feat(api)!: rename").unwrap();
        assert_eq!(breaking.kind, "feat");
        assert!(breaking.breaking);
        assert!(conventional("Update README").is_none());
        assert!(conventional("Feat: capital").is_none());
        assert!(conventional("feat(: odd").is_none());
        assert!(!conventional("fix: plain").unwrap().breaking);
    }

    #[test]
    fn has_content_ignores_the_upstream_line_and_headings() {
        assert!(!has_content(
            "\nUpstream: Python SDK 1.0.0, CLI 2.0.0\n\n### Added\n\n"
        ));
        assert!(has_content(
            "\nUpstream: Python SDK 1.0.0, CLI 2.0.0\n\n- One.\n"
        ));
    }
}
