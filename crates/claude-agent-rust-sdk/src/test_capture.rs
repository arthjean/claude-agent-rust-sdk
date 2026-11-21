//! Captures the `tracing` events of one level emitted on the current
//! thread. Compiled for the unit tests of the crate and loaded by the
//! integration tests through `#[path]`, so it stays inside the package.
//!
//! One collector is the global default subscriber of the test binary, and
//! each [`Warnings`] is a sink of the thread that installed it. A
//! subscriber scoped to the installing thread would lose events: `tracing`
//! caches, per callsite, whether any subscriber is interested, and while
//! one dispatcher exists it asks the default of the thread that reaches the
//! callsite first. A test running on another thread, with no subscriber,
//! would then switch the callsite off for the test that captures.

use std::cell::RefCell;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, Once};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::Interest;
use tracing::{Event, Level, Metadata, Subscriber};

thread_local! {
    /// The sinks installed on this thread, the innermost last.
    static SINKS: RefCell<Vec<(Level, Warnings)>> = const { RefCell::new(Vec::new()) };
}

static COLLECTOR: Once = Once::new();

#[derive(Clone, Default)]
pub struct Warnings(Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<Tagged>>>);

/// The target, the `code` field, and the message of one captured event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tagged {
    pub target: String,
    pub code: Option<String>,
    pub message: String,
}

/// Stops the capture of its [`Warnings`] when dropped. It belongs to the
/// thread that installed it.
pub struct Guard {
    sink: Warnings,
    thread: PhantomData<*const ()>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = SINKS.try_with(|sinks| {
            sinks
                .borrow_mut()
                .retain(|(_, sink)| !Arc::ptr_eq(&sink.0, &self.sink.0));
        });
    }
}

impl Warnings {
    pub fn install() -> (Self, Guard) {
        Self::install_at(Level::WARN)
    }

    pub fn install_at(level: Level) -> (Self, Guard) {
        COLLECTOR.call_once(|| {
            tracing::subscriber::set_global_default(Collector).unwrap();
            tracing::callsite::rebuild_interest_cache();
        });
        let warnings = Self::default();
        SINKS.with(|sinks| sinks.borrow_mut().push((level, warnings.clone())));
        let guard = Guard {
            sink: warnings.clone(),
            thread: PhantomData,
        };
        (warnings, guard)
    }

    pub fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    /// Every captured event with its target and `code` field.
    pub fn tagged(&self) -> Vec<Tagged> {
        self.1.lock().unwrap().clone()
    }

    pub fn about_the_cli_version(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|message| {
                message.contains("too old to report its version")
                    || message.contains("verbatim_prompts is enabled")
                    || message.contains("the version this release of the SDK is tested against")
            })
            .cloned()
            .collect()
    }
}

struct Collector;

struct Message(String, Option<String>);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        match field.name() {
            "message" => self.0 = format!("{value:?}"),
            "code" => self.1 = Some(format!("{value:?}")),
            _ => {}
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.0 = value.to_owned(),
            "code" => self.1 = Some(value.to_owned()),
            _ => {}
        }
    }
}

impl Subscriber for Collector {
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }

    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _attributes: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let innermost = SINKS
            .try_with(|sinks| sinks.borrow().last().cloned())
            .ok()
            .flatten();
        let Some((level, sink)) = innermost else {
            return;
        };
        if *event.metadata().level() != level {
            return;
        }
        let mut message = Message(String::new(), None);
        event.record(&mut message);
        sink.1.lock().unwrap().push(Tagged {
            target: event.metadata().target().to_owned(),
            code: message.1,
            message: message.0.clone(),
        });
        sink.0.lock().unwrap().push(message.0);
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}
