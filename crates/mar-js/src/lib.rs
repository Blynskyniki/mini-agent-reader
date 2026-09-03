//! A page that runs JavaScript without a renderer, a layout engine or a
//! compositor.
//!
//! The pieces: an arena DOM from `mar-dom`, a QuickJS context, a thin native
//! bridge (`natives`), a JavaScript prelude that builds the rest of the browser
//! environment, and a settle loop on a virtual clock (`page`).

pub mod dom;
pub mod modules;
pub mod natives;
pub mod net;
pub mod page;
pub mod state;
pub mod timers;

pub use dom::{DomNode, SharedDoc};
pub use net::{HttpRequest, HttpResponse, NetworkProvider, NoNetwork, StaticNetwork};
pub use page::{Page, PageError, PageOutcome};
pub use state::{ConsoleMessage, Limits, LogLevel, PageState, ScriptError, default_user_agent};
