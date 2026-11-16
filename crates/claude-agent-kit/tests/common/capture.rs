//! Captures the `tracing` events of one level emitted on the current
//! thread. Shared by the integration tests and, through `#[path]`, by unit
//! tests of the crate.
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
pub struct Warnings(Arc<Mutex<Vec<String>>>);

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

    pub fn about_the_cli_version(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|message| {
                message.contains("too old to report its version")
                    || message.contains("verbatim_prompts is enabled")
            })
            .cloned()
            .collect()
    }
}

struct Collector;

struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
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
        let mut message = Message(String::new());
        event.record(&mut message);
        sink.0.lock().unwrap().push(message.0);
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}
