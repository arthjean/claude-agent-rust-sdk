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
const TRACE_PARENT_KEY: &str = "traceparent";
const INHERITED_TRACE_VARIABLES: [&str; 2] = ["TRACEPARENT", "TRACESTATE"];

/// Keys and values the OpenTelemetry propagator writes for the context
/// active on the calling thread. Empty without the `otel` feature, and when
/// the propagator panics.
///
/// The global propagator of the application is used when one is
/// installed. Without one, the `opentelemetry` crate falls back to a
/// propagator that declares no field and writes nothing, where the Python
/// SDK writes the W3C trace context and baggage, so this writes those two
/// itself, following `OTEL_PROPAGATORS` as the Python SDK does.
#[cfg(feature = "otel")]
pub(crate) fn active_trace_context() -> Vec<(String, String)> {
    let injected = std::panic::catch_unwind(|| {
        let context = opentelemetry::Context::current();
        let mut carrier = std::collections::HashMap::new();
        let installed = opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&context, &mut carrier);
            propagator.fields().next().is_some()
        });
        if carrier.is_empty() && !installed {
            let configured = std::env::var(PROPAGATORS_VARIABLE).ok();
            w3c::inject(&context, configured.as_deref(), &mut carrier);
        }
        carrier
    });
    match injected {
        Ok(carrier) => carrier.into_iter().collect(),
        Err(_) => {
            tracing::debug!("OTEL trace context injection failed");
            Vec::new()
        }
    }
}

#[cfg(not(feature = "otel"))]
pub(crate) fn active_trace_context() -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(feature = "otel")]
const PROPAGATORS_VARIABLE: &str = "OTEL_PROPAGATORS";

/// The propagators the Python SDK installs by default, W3C trace context
/// and W3C baggage, written against the OpenTelemetry API alone, as a
/// library should depend on it.
#[cfg(feature = "otel")]
mod w3c {
    use std::collections::HashMap;
    use std::fmt::Write as _;

    use opentelemetry::Context;
    use opentelemetry::baggage::BaggageExt;
    use opentelemetry::trace::{TraceContextExt, TraceFlags};

    const DEFAULT_PROPAGATORS: &str = "tracecontext,baggage";

    /// Writes the context with the propagators `configured` names, in the
    /// syntax of `OTEL_PROPAGATORS`: `tracecontext` and `baggage` are
    /// written, `none` turns propagation off, and any other name is
    /// skipped. Unset, both are written.
    pub(super) fn inject(
        context: &Context,
        configured: Option<&str>,
        carrier: &mut HashMap<String, String>,
    ) {
        let names: Vec<&str> = configured
            .unwrap_or(DEFAULT_PROPAGATORS)
            .split(',')
            .map(str::trim)
            .collect();
        if names.iter().any(|name| name.eq_ignore_ascii_case("none")) {
            return;
        }
        for name in names {
            match name {
                "tracecontext" => trace_context(context, carrier),
                "baggage" => baggage(context, carrier),
                "" => {}
                other => tracing::debug!(
                    "Skipping the OpenTelemetry propagator {other:?}: only tracecontext and \
                     baggage are written without a global propagator"
                ),
            }
        }
    }

    /// The `traceparent` and `tracestate` fields of the W3C trace context.
    /// An empty trace state is left out, as the Python propagator does.
    fn trace_context(context: &Context, carrier: &mut HashMap<String, String>) {
        let span = context.span();
        let span_context = span.span_context();
        if !span_context.is_valid() {
            return;
        }
        carrier.insert(
            "traceparent".to_owned(),
            format!(
                "00-{}-{}-{:02x}",
                span_context.trace_id(),
                span_context.span_id(),
                span_context.trace_flags() & TraceFlags::SAMPLED
            ),
        );
        let state = span_context.trace_state().header();
        if !state.is_empty() {
            carrier.insert("tracestate".to_owned(), state);
        }
    }

    /// The `baggage` field of W3C baggage: `name=value` pairs joined by
    /// commas and encoded as the Python propagator encodes them, each with
    /// its metadata, which Python baggage cannot hold, after a semicolon.
    fn baggage(context: &Context, carrier: &mut HashMap<String, String>) {
        let baggage = context.baggage();
        if baggage.is_empty() {
            return;
        }
        let entries: Vec<String> = baggage
            .iter()
            .map(|(name, (value, metadata))| {
                let mut entry = quote_plus(name.as_str());
                entry.push('=');
                entry.push_str(&quote_plus(value.as_str()));
                let metadata = metadata.as_str().trim();
                if !metadata.is_empty() {
                    entry.push(';');
                    entry.push_str(metadata);
                }
                entry
            })
            .collect();
        carrier.insert("baggage".to_owned(), entries.join(","));
    }

    /// Encodes text as Python's `urllib.parse.quote_plus`: the unreserved
    /// characters of RFC 3986 stay, a space becomes `+`, and every other
    /// byte of the UTF-8 text is percent-encoded.
    fn quote_plus(text: &str) -> String {
        let mut encoded = String::with_capacity(text.len());
        for byte in text.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                encoded.push(char::from(byte));
            } else if byte == b' ' {
                encoded.push('+');
            } else {
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
        encoded
    }
}

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
    trace_context: &[(String, String)],
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

    if trace_context.iter().any(|(key, _)| key == TRACE_PARENT_KEY) {
        for name in INHERITED_TRACE_VARIABLES {
            if !options.env.contains_key(name) {
                environment.remove(name);
            }
        }
        for (key, value) in trace_context {
            let name = key.to_uppercase();
            if !options.env.contains_key(&name) {
                environment.set(name, value);
            }
        }
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
        build_traced(options, ambient, platform, &[])
    }

    fn build_traced(
        options: &ClaudeAgentOptions,
        ambient: &[(&str, &str)],
        platform: Platform,
        trace_context: &[(&str, &str)],
    ) -> Vec<(String, String)> {
        let inherited = ambient
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        let trace_context: Vec<(String, String)> = trace_context
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        build_environment(options, inherited, platform, &trace_context)
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

    const TRACE_PARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

    #[test]
    fn trace_context_is_written_with_upper_case_names() {
        let options = ClaudeAgentOptions::default();
        let entries = build_traced(
            &options,
            &[],
            Platform::Unix,
            &[
                ("traceparent", TRACE_PARENT),
                ("tracestate", "vendor=value"),
                ("baggage", "tenant=acme"),
            ],
        );
        assert_eq!(get(&entries, "TRACEPARENT"), Some(TRACE_PARENT));
        assert_eq!(get(&entries, "TRACESTATE"), Some("vendor=value"));
        assert_eq!(get(&entries, "BAGGAGE"), Some("tenant=acme"));
        assert_eq!(get(&entries, "traceparent"), None);
    }

    #[test]
    fn inherited_trace_variables_are_removed_before_the_fresh_values() {
        let options = ClaudeAgentOptions::default();
        let entries = build_traced(
            &options,
            &[
                ("TRACEPARENT", "00-stale-stale-00"),
                ("TRACESTATE", "stale=1"),
            ],
            Platform::Unix,
            &[("traceparent", TRACE_PARENT)],
        );
        assert_eq!(get(&entries, "TRACEPARENT"), Some(TRACE_PARENT));
        assert_eq!(get(&entries, "TRACESTATE"), None);
    }

    #[test]
    fn trace_variables_of_the_options_are_never_overwritten() {
        let options = with_variables(&[
            ("TRACEPARENT", "00-explicit-explicit-01"),
            ("BAGGAGE", "explicit=1"),
        ])
        .build()
        .unwrap();
        let entries = build_traced(
            &options,
            &[("TRACESTATE", "stale=1")],
            Platform::Unix,
            &[
                ("traceparent", TRACE_PARENT),
                ("tracestate", "vendor=value"),
                ("baggage", "tenant=acme"),
            ],
        );
        assert_eq!(
            get(&entries, "TRACEPARENT"),
            Some("00-explicit-explicit-01")
        );
        assert_eq!(get(&entries, "TRACESTATE"), Some("vendor=value"));
        assert_eq!(get(&entries, "BAGGAGE"), Some("explicit=1"));

        let options = with_variables(&[("TRACESTATE", "explicit=1")])
            .build()
            .unwrap();
        let entries = build_traced(
            &options,
            &[("TRACESTATE", "stale=1")],
            Platform::Unix,
            &[("traceparent", TRACE_PARENT)],
        );
        assert_eq!(get(&entries, "TRACESTATE"), Some("explicit=1"));
    }

    #[test]
    fn without_a_trace_parent_inherited_trace_variables_are_left_untouched() {
        let options = ClaudeAgentOptions::default();
        let ambient = [
            ("TRACEPARENT", "00-inherited-inherited-01"),
            ("TRACESTATE", "inherited=1"),
        ];
        let baggage_only = [("baggage", "tenant=acme")];
        for trace_context in [&[][..], &baggage_only[..]] {
            let entries = build_traced(&options, &ambient, Platform::Unix, trace_context);
            assert_eq!(
                get(&entries, "TRACEPARENT"),
                Some("00-inherited-inherited-01")
            );
            assert_eq!(get(&entries, "TRACESTATE"), Some("inherited=1"));
            assert_eq!(get(&entries, "BAGGAGE"), None);
        }
    }

    #[test]
    fn the_client_app_option_wins_over_the_variable_set_in_the_options() {
        let options = with_variables(&[(EXPLICIT_CLIENT_APP_VARIABLE, "from-env/1.0")])
            .client_app("from-option/2.0")
            .build()
            .unwrap();
        let entries = build(
            &options,
            &[(DERIVED_CLIENT_APP_VARIABLE, "outer-app")],
            Platform::Unix,
        );
        assert_eq!(
            get(&entries, EXPLICIT_CLIENT_APP_VARIABLE),
            Some("from-option/2.0")
        );
        assert_eq!(get(&entries, DERIVED_CLIENT_APP_VARIABLE), None);
    }

    #[test]
    fn a_client_app_variable_set_in_the_options_reaches_the_cli_unchanged() {
        let options = with_variables(&[(EXPLICIT_CLIENT_APP_VARIABLE, "from-env/1.0")])
            .build()
            .unwrap();
        let entries = build(&options, &[], Platform::Unix);
        assert_eq!(
            get(&entries, EXPLICIT_CLIENT_APP_VARIABLE),
            Some("from-env/1.0")
        );
        assert_eq!(get(&entries, DERIVED_CLIENT_APP_VARIABLE), None);
    }

    #[test]
    fn an_empty_client_app_option_is_sent_as_an_empty_value() {
        let options = ClaudeAgentOptions::builder()
            .client_app("")
            .build()
            .unwrap();
        let entries = build(
            &options,
            &[(EXPLICIT_CLIENT_APP_VARIABLE, "ambient-app")],
            Platform::Unix,
        );
        assert_eq!(get(&entries, EXPLICIT_CLIENT_APP_VARIABLE), Some(""));
        assert_eq!(get(&entries, DERIVED_CLIENT_APP_VARIABLE), None);
    }
}
