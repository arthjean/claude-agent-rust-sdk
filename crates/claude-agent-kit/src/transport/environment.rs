//! Construction of the environment of the CLI process.

use std::ffi::{OsStr, OsString};

use crate::cli::Platform;
use crate::types::ClaudeAgentOptions;

pub(crate) const ENTRYPOINT_VARIABLE: &str = "CLAUDE_CODE_ENTRYPOINT";
pub(crate) const ENTRYPOINT: &str = "sdk-rs";
pub(crate) const SDK_VERSION_VARIABLE: &str = "CLAUDE_AGENT_SDK_VERSION";
pub(crate) const EXPLICIT_CLIENT_APP_VARIABLE: &str = "CLAUDE_AGENT_SDK_CLIENT_APP";
pub(crate) const DERIVED_CLIENT_APP_VARIABLE: &str = "CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP";
pub(crate) const SDK_READS_SESSION_STATE_VARIABLE: &str = "CLAUDE_CODE_SDK_READS_SESSION_STATE";
pub(crate) const FILE_CHECKPOINTING_VARIABLE: &str = "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING";
const NESTED_SESSION_VARIABLE: &str = "CLAUDECODE";
const WORKING_DIRECTORY_VARIABLE: &str = "PWD";

#[derive(Debug)]
pub(crate) struct Environment {
    platform: Platform,
    entries: Vec<(OsString, OsString)>,
}

impl Environment {
    fn new(platform: Platform) -> Self {
        Self {
            platform,
            entries: Vec::new(),
        }
    }

    fn same_name(&self, left: &OsStr, right: &OsStr) -> bool {
        match self.platform {
            Platform::Windows => left.eq_ignore_ascii_case(right),
            Platform::Unix => left == right,
        }
    }

    fn position(&self, name: &OsStr) -> Option<usize> {
        self.entries
            .iter()
            .position(|(existing, _)| self.same_name(existing, name))
    }

    fn set(&mut self, name: impl Into<OsString>, value: impl Into<OsString>) {
        let name = name.into();
        match self.position(&name) {
            Some(index) => self.entries[index] = (name, value.into()),
            None => self.entries.push((name, value.into())),
        }
    }

    fn remove(&mut self, name: &str) {
        if let Some(index) = self.position(OsStr::new(name)) {
            self.entries.remove(index);
        }
    }

    fn contains_in_any_case(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|(existing, _)| existing.eq_ignore_ascii_case(name))
    }

    pub(crate) fn into_entries(self) -> Vec<(OsString, OsString)> {
        self.entries
    }
}

pub(crate) fn build_environment(
    options: &ClaudeAgentOptions,
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
    platform: Platform,
) -> Environment {
    let mut environment = Environment::new(platform);
    for (name, value) in inherited {
        environment.set(name, value);
    }
    environment.remove(NESTED_SESSION_VARIABLE);
    environment.set(ENTRYPOINT_VARIABLE, ENTRYPOINT);
    for (name, value) in &options.env {
        environment.set(name, value);
    }
    environment.set(SDK_VERSION_VARIABLE, crate::VERSION);

    environment.remove(DERIVED_CLIENT_APP_VARIABLE);
    if let Some(client_app) = &options.client_app {
        environment.set(EXPLICIT_CLIENT_APP_VARIABLE, client_app);
    }

    if !environment.contains_in_any_case(SDK_READS_SESSION_STATE_VARIABLE) {
        environment.set(SDK_READS_SESSION_STATE_VARIABLE, "1");
    }
    if options.enable_file_checkpointing {
        environment.set(FILE_CHECKPOINTING_VARIABLE, "true");
    }
    if let Some(cwd) = &options.cwd
        && !cwd.as_os_str().is_empty()
    {
        environment.set(WORKING_DIRECTORY_VARIABLE, cwd.as_os_str());
    }
    environment
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{
        DERIVED_CLIENT_APP_VARIABLE, ENTRYPOINT_VARIABLE, EXPLICIT_CLIENT_APP_VARIABLE,
        FILE_CHECKPOINTING_VARIABLE, SDK_READS_SESSION_STATE_VARIABLE, SDK_VERSION_VARIABLE,
        build_environment,
    };
    use crate::cli::Platform;
    use crate::types::{ClaudeAgentOptions, ClaudeAgentOptionsBuilder};

    type Variables = &'static [(&'static str, &'static str)];

    fn with_variables(variables: &[(&str, &str)]) -> ClaudeAgentOptionsBuilder {
        variables
            .iter()
            .fold(ClaudeAgentOptions::builder(), |builder, (name, value)| {
                builder.env_var(*name, *value)
            })
    }

    fn build(
        options: &ClaudeAgentOptions,
        ambient: &[(&str, &str)],
        platform: Platform,
    ) -> Vec<(String, String)> {
        let inherited = ambient
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        build_environment(options, inherited, platform)
            .into_entries()
            .into_iter()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect()
    }

    fn get<'a>(entries: &'a [(String, String)], name: &str) -> Option<&'a str> {
        entries
            .iter()
            .find(|(existing, _)| existing == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn layers_apply_in_the_upstream_order() {
        let options = with_variables(&[
            ("MY_TEST_VAR", "from-options"),
            ("SHARED", "from-options"),
            ("CLAUDE_AGENT_SDK_VERSION", "0.0.0"),
        ])
        .build()
        .unwrap();
        let entries = build(
            &options,
            &[
                ("PATH", "/usr/bin"),
                ("SHARED", "from-ambient"),
                ("CLAUDECODE", "1"),
                ("CLAUDE_CODE_ENTRYPOINT", "cli"),
                (DERIVED_CLIENT_APP_VARIABLE, "outer-app"),
            ],
            Platform::Unix,
        );
        assert_eq!(get(&entries, "PATH"), Some("/usr/bin"));
        assert_eq!(get(&entries, "MY_TEST_VAR"), Some("from-options"));
        assert_eq!(get(&entries, "SHARED"), Some("from-options"));
        assert_eq!(get(&entries, "CLAUDECODE"), None);
        assert_eq!(get(&entries, ENTRYPOINT_VARIABLE), Some("sdk-rs"));
        assert_eq!(get(&entries, SDK_VERSION_VARIABLE), Some(crate::VERSION));
        assert_eq!(get(&entries, DERIVED_CLIENT_APP_VARIABLE), None);
        assert_eq!(get(&entries, EXPLICIT_CLIENT_APP_VARIABLE), None);
        assert_eq!(get(&entries, FILE_CHECKPOINTING_VARIABLE), None);
        assert_eq!(get(&entries, "PWD"), None);
    }

    #[test]
    fn caller_can_override_the_entrypoint_and_set_claudecode() {
        let options = with_variables(&[
            ("CLAUDE_CODE_ENTRYPOINT", "custom-caller"),
            ("CLAUDECODE", "1"),
        ])
        .build()
        .unwrap();
        let entries = build(&options, &[("CLAUDECODE", "outer")], Platform::Unix);
        assert_eq!(get(&entries, ENTRYPOINT_VARIABLE), Some("custom-caller"));
        assert_eq!(get(&entries, "CLAUDECODE"), Some("1"));
        assert_eq!(get(&entries, SDK_VERSION_VARIABLE), Some(crate::VERSION));
    }

    #[test]
    fn session_state_frames_are_requested_unless_the_caller_chose() {
        let cases: [(Variables, Variables, Option<&str>); 4] = [
            (&[], &[], Some("1")),
            (&[(SDK_READS_SESSION_STATE_VARIABLE, "0")], &[], Some("0")),
            (&[], &[(SDK_READS_SESSION_STATE_VARIABLE, "0")], Some("0")),
            (&[("claude_code_sdk_reads_session_state", "0")], &[], None),
        ];
        for (from_options, ambient, expected) in cases {
            let options = with_variables(from_options).build().unwrap();
            let entries = build(&options, ambient, Platform::Unix);
            assert_eq!(get(&entries, SDK_READS_SESSION_STATE_VARIABLE), expected);
            assert_eq!(get(&entries, "CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS"), None);
        }
    }

    #[test]
    fn optional_variables_follow_their_options() {
        let options = ClaudeAgentOptions::builder()
            .enable_file_checkpointing(true)
            .cwd("/work/project")
            .client_app("my-app/1.0.0")
            .build()
            .unwrap();
        let entries = build(
            &options,
            &[(EXPLICIT_CLIENT_APP_VARIABLE, "ambient-app")],
            Platform::Unix,
        );
        assert_eq!(get(&entries, FILE_CHECKPOINTING_VARIABLE), Some("true"));
        assert_eq!(get(&entries, "PWD"), Some("/work/project"));
        assert_eq!(
            get(&entries, EXPLICIT_CLIENT_APP_VARIABLE),
            Some("my-app/1.0.0")
        );
    }

    #[test]
    fn explicit_client_app_can_come_from_the_ambient_variables() {
        let options = ClaudeAgentOptions::default();
        let entries = build(
            &options,
            &[(EXPLICIT_CLIENT_APP_VARIABLE, "ambient-app")],
            Platform::Unix,
        );
        assert_eq!(
            get(&entries, EXPLICIT_CLIENT_APP_VARIABLE),
            Some("ambient-app")
        );

        let options = with_variables(&[(EXPLICIT_CLIENT_APP_VARIABLE, "")])
            .build()
            .unwrap();
        let entries = build(&options, &[], Platform::Unix);
        assert_eq!(get(&entries, EXPLICIT_CLIENT_APP_VARIABLE), Some(""));
    }

    #[test]
    fn windows_names_are_matched_without_regard_to_case() {
        let options = with_variables(&[("path", "C:\\override")]).build().unwrap();
        let entries = build(
            &options,
            &[("Path", "C:\\Windows"), ("ClaudeCode", "1")],
            Platform::Windows,
        );
        assert_eq!(get(&entries, "path"), Some("C:\\override"));
        assert_eq!(get(&entries, "Path"), None);
        assert_eq!(get(&entries, "ClaudeCode"), None);
    }
}
