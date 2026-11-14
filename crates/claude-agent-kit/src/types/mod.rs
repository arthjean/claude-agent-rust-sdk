//! Typed data model: everything that crosses the wire, plus the options.

use std::future::Future;
use std::pin::Pin;

pub(crate) mod macros;

mod config;
mod content;
mod hook;
mod message;
mod options;
mod permission;
mod status;

pub use config::*;
pub use content::*;
pub use hook::*;
pub use message::*;
pub use options::*;
pub use permission::*;
pub use status::*;

/// Boxed future returned by stored callbacks.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
