use crate::id::OrderId;

/// One status value the TWS wire can carry, after parsing.
///
/// These are the nine values the Java and C++ client enums define, plus
/// [`Phase::Unknown`] for a string no client enum names. `ApiPending` and
/// `ApiCancelled` exist only in those two enums; the Python wrapper never
/// mentions them.
///
/// The proto that carries the same field also lists `PreCancelled` and
/// `WarnState`, which appear in no client enum. They fall into `Unknown` and
/// the tracker ignores them: the protocol disagrees with itself here, and
/// inventing a phase for a value no client sends would encode a guess.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Phase {
    /// Set locally by the client when it writes the order. TWS never sends it.
    PendingSubmit,
    /// Set locally by the client when it asks to cancel. TWS never sends it.
    PendingCancel,
    ApiPending,
    PreSubmitted,
    Submitted,
    Cancelled,
    /// The order's balance is cancelled, without a status the client asked for.
    ApiCancelled,
    Filled,
    /// Accepted by IB but parked by a system or exchange condition.
    Inactive,
    Unknown,
}

impl Phase {
    /// Maps the wire string onto a phase, keeping unrecognised values as
    /// [`Phase::Unknown`] so a vendor drop that adds a status cannot panic.
    #[must_use]
    pub fn from_wire(status: &str) -> Self {
        match status {
            "PendingSubmit" => Self::PendingSubmit,
            "PendingCancel" => Self::PendingCancel,
            "ApiPending" => Self::ApiPending,
            "PreSubmitted" => Self::PreSubmitted,
            "Submitted" => Self::Submitted,
            "Cancelled" => Self::Cancelled,
            "ApiCancelled" => Self::ApiCancelled,
            "Filled" => Self::Filled,
            "Inactive" => Self::Inactive,
            _ => Self::Unknown,
        }
    }

    /// The order can still change on its own, per IBKR's own helper.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::PreSubmitted | Self::PendingCancel | Self::Submitted | Self::PendingSubmit
        )
    }

    /// The order has reached a state no later report can move it out of.
    ///
    /// IBKR's helper also lists `Inactive` here, and this model does not. The
    /// `orderStatus` documentation calls an inactive order one that
    /// "currently" cannot work, which is a wait, not an end: a held order
    /// resumes when the halt or the exchange condition clears, and a client
    /// that treated it as final would drop the resume. So `Inactive` is not
    /// active and not terminal, and the tracker keeps it in
    /// [`crate::tracker::LivePhase`] so a later report can move it on.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Filled | Self::Cancelled | Self::ApiCancelled)
    }

    /// TWS documents both of these as client-set only and never sends them.
    #[must_use]
    pub const fn is_client_set(self) -> bool {
        matches!(self, Self::PendingSubmit | Self::PendingCancel)
    }
}

/// A count of shares.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Shares(u64);

impl Shares {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn new(count: u64) -> Self {
        Self(count)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Sums two counts, or `None` past `u64::MAX`.
    ///
    /// The tracker never adds fills, because the wire reports cumulative
    /// totals. Callers that do aggregate fills get an explicit overflow
    /// instead of a wrapped quantity.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(sum) => Some(Self(sum)),
            None => None,
        }
    }

    #[must_use]
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

/// One inbound report, in the broker's vocabulary rather than the wire's.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Report {
    Status {
        order_id: OrderId,
        phase: Phase,
        /// Cumulative shares filled so far, not a delta.
        filled: Shares,
        remaining: Shares,
    },
    Error {
        order_id: Option<OrderId>,
        code: i32,
        message: String,
    },
}

impl Report {
    /// The order this report is about, when it names one.
    #[must_use]
    pub const fn order_id(&self) -> Option<OrderId> {
        match self {
            Self::Status { order_id, .. } => Some(*order_id),
            Self::Error { order_id, .. } => *order_id,
        }
    }
}
