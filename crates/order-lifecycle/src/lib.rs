//! The lifecycle of an Interactive Brokers order, seen from a TWS API client.
//!
//! The crate is three layers. [`report`] and [`id`] hold the broker's
//! vocabulary as types. [`boundary`] is the only place that names
//! `tws_api::proto`, and it parses wire messages into those types. [`tracker`]
//! is the reducer that turns a report into a new belief about an order, and
//! [`checker`] is the Stateright model that proves its rules.
//!
//! Out of scope: bracket and OCA orders, `parent_id` linkage, and any socket or
//! async client. Nothing here opens a connection.

// Every fallible function here returns this crate's own error type, whose
// variants the signature already names, so an `# Errors` section would only
// restate them.
#![allow(clippy::missing_errors_doc)]

pub mod boundary;
pub mod checker;
pub mod id;
pub mod report;
pub mod tracker;

pub use boundary::WireError;
pub use id::OrderId;
pub use report::{Phase, Report, Shares};
pub use tracker::{
    Applied, ApplyError, IgnoredReport, LivePhase, Order, OrderSlot, Outcome, Snapshot, Strategy,
    TrackedPhase,
};
