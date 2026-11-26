use crate::id::OrderId;
use crate::report::{Phase, Report, Shares};

/// Bound on [`Order::log`].
///
/// A report that would overflow the log is refused with
/// [`ApplyError::LogFull`], not applied with its entry dropped. Every invariant
/// in [`crate::checker`] reads the log as the order's whole history, so a log
/// that quietly stopped recording would let a rule agree with a state it could
/// no longer explain. Refusing also bounds the model, since a reconnect loop
/// otherwise grows the log forever.
pub const MAX_LOG: usize = 5;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LivePhase {
    ApiPending,
    PreSubmitted,
    Submitted,
    /// Accepted, but parked by a system or exchange condition.
    Inactive,
}

impl LivePhase {
    #[must_use]
    pub const fn from_phase(phase: Phase) -> Option<Self> {
        match phase {
            Phase::ApiPending => Some(Self::ApiPending),
            Phase::PreSubmitted => Some(Self::PreSubmitted),
            Phase::Submitted => Some(Self::Submitted),
            Phase::Inactive => Some(Self::Inactive),
            Phase::Unknown
            | Phase::PendingSubmit
            | Phase::PendingCancel
            | Phase::Cancelled
            | Phase::ApiCancelled
            | Phase::Filled => None,
        }
    }

    #[must_use]
    pub const fn as_phase(self) -> Phase {
        match self {
            Self::ApiPending => Phase::ApiPending,
            Self::PreSubmitted => Phase::PreSubmitted,
            Self::Submitted => Phase::Submitted,
            Self::Inactive => Phase::Inactive,
        }
    }

    /// Position in the acceptance order, or `None` for [`Self::Inactive`],
    /// which sits outside it because an order leaves a hold into whichever
    /// phase it held before.
    #[must_use]
    pub const fn rank(self) -> Option<u8> {
        match self {
            Self::ApiPending => Some(0),
            Self::PreSubmitted => Some(1),
            Self::Submitted => Some(2),
            Self::Inactive => None,
        }
    }

    /// Whether a report naming `next` may move an order out of `self`.
    ///
    /// The client re-receives every open order's snapshot after a reconnect,
    /// and those snapshots can arrive behind a newer one. A report naming a
    /// phase the order already left is stale, and following it would invent a
    /// regression the broker never reported. A hold and its release are the
    /// exception, since they move in both directions by design.
    #[must_use]
    pub const fn advances_to(self, next: Self) -> bool {
        match (self.rank(), next.rank()) {
            (Some(current), Some(reported)) => reported >= current,
            _ => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Outcome {
    Filled,
    Cancelled,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TrackedPhase {
    /// Written by the client and transmitted, not yet acknowledged by TWS.
    PendingSubmit,
    Live(LivePhase),
    /// The client asked to cancel and waits for an answer. TWS never reports
    /// this phase, so only [`Order::request_cancel`] can enter it.
    PendingCancel,
    Done(Outcome),
}

impl TrackedPhase {
    #[must_use]
    pub const fn is_done(self) -> bool {
        matches!(self, Self::Done(_))
    }

    /// The wire phase a reconnect replays for this state.
    ///
    /// A rejection has no status of its own. It arrives as an error, and IB
    /// answers a reconnect with nothing rather than with a `Cancelled`.
    #[must_use]
    pub const fn as_phase(self) -> Option<Phase> {
        match self {
            Self::PendingSubmit => Some(Phase::PendingSubmit),
            Self::Live(live) => Some(live.as_phase()),
            Self::PendingCancel => Some(Phase::PendingCancel),
            Self::Done(Outcome::Filled) => Some(Phase::Filled),
            Self::Done(Outcome::Cancelled) => Some(Phase::Cancelled),
            Self::Done(Outcome::Rejected) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Snapshot {
    pub phase: TrackedPhase,
    pub filled: Shares,
}

impl Snapshot {
    const fn new(phase: TrackedPhase, filled: Shares) -> Self {
        Self { phase, filled }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IgnoredReport {
    LateAfterDone,
    /// `PendingSubmit` or `PendingCancel`, which the client sets and TWS never
    /// sends.
    ClientSetPhase(Phase),
    UnknownPhase,
    UnhandledError {
        code: i32,
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Applied {
    Placed,
    CancelRequested,
    PhaseReport(Phase),
    /// The cumulative fill exactly as the report stated it, never a delta.
    Filled(Shares),
    Rejected {
        code: i32,
    },
    CancelledByBroker {
        code: Option<i32>,
    },
    /// A report that changed nothing, and the state that found it.
    ///
    /// Carrying the snapshot is what lets `duplicate_report_changes_nothing` in
    /// [`crate::checker`] be a predicate on one state. Nothing else reads it.
    Duplicate {
        at: Snapshot,
    },
    Ignored(IgnoredReport),
}

impl Applied {
    /// Whether the broker ended the order, as opposed to the order ending
    /// because its last share filled.
    ///
    /// The two readings matter to the checker's rules, which keep the quantity
    /// and the closing status apart so one broken reducer fails one rule.
    #[must_use]
    pub const fn broker_ended(&self) -> bool {
        match self {
            Self::PhaseReport(phase) => phase.is_terminal(),
            Self::Rejected { .. } | Self::CancelledByBroker { .. } => true,
            Self::Placed
            | Self::CancelRequested
            | Self::Filled(_)
            | Self::Duplicate { .. }
            | Self::Ignored(_) => false,
        }
    }
}

/// Why the tracker refused a report or a command.
///
/// Every variant leaves the order exactly as it was.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyError {
    UnknownOrder(Option<OrderId>),
    AlreadyDone,
    InconsistentFill { reported: Shares, total: Shares },
    LogFull,
    DuplicateOrder(OrderId),
}

impl core::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownOrder(Some(id)) => write!(f, "order {} was never placed", id.get()),
            Self::UnknownOrder(None) => f.write_str("report names no order"),
            Self::AlreadyDone => f.write_str("order already finished"),
            Self::InconsistentFill { reported, total } => write!(
                f,
                "reported fill {} does not complete an order of {}",
                reported.get(),
                total.get()
            ),
            Self::LogFull => f.write_str("order has no room for another log entry"),
            Self::DuplicateOrder(id) => write!(f, "order {} is already placed", id.get()),
        }
    }
}

impl core::error::Error for ApplyError {}

/// Which reducer the tracker runs.
///
/// The lifecycle is the product, so its known deviations live beside it as one
/// changed expression each, rather than as a second reducer inside the model.
/// [`crate::checker`] runs one variant per model and asserts that the property
/// named for it catches it. Production callers only ever get
/// [`Strategy::Correct`], the default.
///
/// The variants after the spec's mutant table are not in it. They exist so that
/// `duplicate_report_changes_nothing`, `cancel_after_done_is_refused`,
/// `no_fill_after_done` and `phase_never_regresses` each have a reducer that can
/// fail them: an invariant no mutation can break proves nothing.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Strategy {
    #[default]
    Correct,
    /// Adds the reported cumulative fill instead of taking the maximum.
    DoubleCountsRedeliveredFill,
    /// Lets a `PreSubmitted` or `Submitted` report move a finished order.
    RevivesAfterDone,
    /// Lets a later cancellation replace the recorded outcome.
    AcceptsSecondOutcome,
    /// Adds one share to a report that should have added nothing.
    OverfillsOnDuplicate,
    /// Rolls the phase back on a report whose entry says nothing changed.
    RephasesOnDuplicate,
    CancelsAfterDone,
    FillsAfterDone,
    /// Follows a snapshot that names a phase the order already passed.
    FollowsStalePhase,
    /// Relabels a finished order with a live phase, keeping the closing entry.
    RelabelsAfterDone,
}

impl Strategy {
    const fn counts_deltas(self) -> bool {
        matches!(self, Self::DoubleCountsRedeliveredFill)
    }

    /// A finished order may be moved back to a live phase.
    const fn revives(self) -> bool {
        matches!(self, Self::RevivesAfterDone)
    }

    /// A finished order may be relabelled with a live phase, entry and all.
    const fn relabels_after_done(self) -> bool {
        matches!(self, Self::RelabelsAfterDone)
    }

    const fn overwrites_outcome(self) -> bool {
        matches!(self, Self::AcceptsSecondOutcome)
    }

    const fn overfills(self) -> bool {
        matches!(self, Self::OverfillsOnDuplicate)
    }

    const fn rephases(self) -> bool {
        matches!(self, Self::RephasesOnDuplicate)
    }

    const fn cancels_after_done(self) -> bool {
        matches!(self, Self::CancelsAfterDone)
    }

    const fn fills_after_done(self) -> bool {
        matches!(self, Self::FillsAfterDone)
    }

    const fn follows_stale(self) -> bool {
        matches!(self, Self::FollowsStalePhase)
    }
}

struct Step {
    applied: Applied,
    at: Snapshot,
}

struct Transition {
    steps: Vec<Step>,
    phase: TrackedPhase,
    filled: Shares,
}

impl Transition {
    fn one(phase: TrackedPhase, filled: Shares, applied: Applied) -> Self {
        let at = Snapshot::new(phase, filled);
        Self {
            steps: vec![Step { applied, at }],
            phase,
            filled,
        }
    }
}

/// What the client believes about one order, and why.
///
/// `history` holds one snapshot per log entry, the state the order held just
/// after that entry. The rules in [`crate::checker`] read it as the order's past;
/// rebuilding that past from the log alone would need a second copy of the
/// reducer's reasoning, and a copy that disagreed with the reducer would make a
/// rule fail for the wrong reason.
///
/// `strategy` is the one field the lifecycle rules do not read: it selects a
/// known deviation so the model checker can prove the invariants catch it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Order {
    phase: TrackedPhase,
    filled: Shares,
    total: Shares,
    log: Vec<Applied>,
    history: Vec<Snapshot>,
    strategy: Strategy,
}

impl Order {
    #[must_use]
    pub fn place(total: Shares) -> Self {
        let at = Snapshot::new(TrackedPhase::PendingSubmit, Shares::ZERO);
        Self {
            phase: TrackedPhase::PendingSubmit,
            filled: Shares::ZERO,
            total,
            log: vec![Applied::Placed],
            history: vec![at],
            strategy: Strategy::Correct,
        }
    }

    #[must_use]
    pub const fn with_strategy(mut self, strategy: Strategy) -> Self {
        self.strategy = strategy;
        self
    }

    #[must_use]
    pub const fn phase(&self) -> TrackedPhase {
        self.phase
    }

    #[must_use]
    pub const fn filled(&self) -> Shares {
        self.filled
    }

    #[must_use]
    pub const fn total(&self) -> Shares {
        self.total
    }

    #[must_use]
    pub fn log(&self) -> &[Applied] {
        &self.log
    }

    /// The state the order held just before log entry `index`.
    ///
    /// Entry zero is preceded by the placed state, which is what makes the
    /// duplicate rule able to compare the first entry against something.
    #[must_use]
    pub fn state_before(&self, index: usize) -> Snapshot {
        index
            .checked_sub(1)
            .and_then(|previous| self.history.get(previous).copied())
            .unwrap_or_else(|| Snapshot::new(TrackedPhase::PendingSubmit, Shares::ZERO))
    }

    /// Every state the order passed through, oldest first.
    ///
    /// A rule about "what happened before" reads this instead of the current
    /// state, which is the whole reason the history is part of the order.
    #[must_use]
    pub fn history(&self) -> &[Snapshot] {
        &self.history
    }

    /// The status a reconnect replays for this order, or `None` when it
    /// finished on an error and has no status to replay.
    #[must_use]
    pub fn snapshot(&self) -> Option<(Phase, Shares, Shares)> {
        let phase = self.phase.as_phase()?;
        Some((phase, self.filled, self.total.saturating_sub(self.filled)))
    }

    /// # Errors
    ///
    /// [`ApplyError::AlreadyDone`] once the order finished, and
    /// [`ApplyError::LogFull`] when the log is full. Both leave it untouched.
    pub fn request_cancel(&mut self) -> Result<&Applied, ApplyError> {
        if self.phase == TrackedPhase::PendingCancel {
            return self.commit(Transition::one(
                self.phase,
                self.filled,
                Applied::Duplicate {
                    at: Snapshot::new(self.phase, self.filled),
                },
            ));
        }
        // The deviation records the request against the phase the order already
        // holds, so the entry is the only thing that contradicts the rule rather
        // than the rule plus a revived phase.
        let phase = if self.phase.is_done() {
            if !self.strategy.cancels_after_done() {
                return Err(ApplyError::AlreadyDone);
            }
            self.phase
        } else {
            TrackedPhase::PendingCancel
        };
        self.commit(Transition::one(
            phase,
            self.filled,
            Applied::CancelRequested,
        ))
    }

    /// # Errors
    ///
    /// [`ApplyError::InconsistentFill`] for a `Filled` report whose quantity
    /// falls short of the order size, and [`ApplyError::LogFull`] when the log
    /// is full. Both leave the order untouched.
    pub fn apply(&mut self, report: &Report) -> Result<&Applied, ApplyError> {
        let transition = self.plan(report)?;
        self.commit(transition)
    }

    fn plan(&self, report: &Report) -> Result<Transition, ApplyError> {
        match report {
            Report::Status { phase, filled, .. } => self.plan_status(*phase, *filled),
            // The single-outcome rule covers errors too. A 202 that arrives behind
            // the fill it cancelled describes an order that already closed, and
            // recording it would hand the order a second outcome.
            Report::Error { .. } if self.phase.is_done() => {
                Ok(self.ignored(IgnoredReport::LateAfterDone))
            }
            Report::Error { code, .. } => Ok(self.plan_error(*code)),
        }
    }

    fn plan_status(&self, reported: Phase, filled: Shares) -> Result<Transition, ApplyError> {
        if self.phase.is_done() {
            return Ok(self
                .revive(reported, filled)
                .unwrap_or_else(|| self.ignored(IgnoredReport::LateAfterDone)));
        }
        if reported.is_client_set() {
            return Ok(self.ignored(IgnoredReport::ClientSetPhase(reported)));
        }
        match reported {
            Phase::Filled => self.plan_filled(filled),
            Phase::Cancelled | Phase::ApiCancelled => Ok(self.plan_cancelled(reported, filled)),
            Phase::ApiPending | Phase::PreSubmitted | Phase::Submitted | Phase::Inactive => {
                Ok(self.plan_live(reported, filled))
            }
            Phase::Unknown => Ok(self.ignored(IgnoredReport::UnknownPhase)),
            Phase::PendingSubmit | Phase::PendingCancel => {
                Ok(self.ignored(IgnoredReport::ClientSetPhase(reported)))
            }
        }
    }

    /// The ways a finished order can still move, each under the strategy that
    /// exists to prove an invariant notices.
    ///
    /// The revival and the outcome overwrite both leave the fill alone, so neither
    /// can also be read as a fill the log fails to explain.
    fn revive(&self, reported: Phase, filled: Shares) -> Option<Transition> {
        // This deviation leaves the closing entry in place and records a live
        // phase after it, so the history re-opens an order the log says closed.
        // It is the only mutant that reaches that arm of `single_outcome`.
        if self.strategy.relabels_after_done()
            && let Some(live) = LivePhase::from_phase(reported)
        {
            return Some(Transition::one(
                TrackedPhase::Live(live),
                self.filled,
                Applied::PhaseReport(reported),
            ));
        }
        if self.strategy.revives()
            && matches!(reported, Phase::PreSubmitted | Phase::Submitted)
            && let Some(live) = LivePhase::from_phase(reported)
        {
            return Some(Transition::one(
                TrackedPhase::Live(live),
                self.filled,
                Applied::PhaseReport(reported),
            ));
        }
        if self.strategy.overwrites_outcome()
            && matches!(reported, Phase::Cancelled | Phase::ApiCancelled)
        {
            return Some(Transition::one(
                TrackedPhase::Done(Outcome::Cancelled),
                self.filled,
                Applied::PhaseReport(reported),
            ));
        }
        // The deviation records whatever a finished order's report carries and
        // leaves the phase alone, so the entry is the only trace it leaves. It
        // records even when the quantity does not advance, which is why it reads
        // a settled amount rather than comparing: a comparison here would have a
        // branch no input can separate from the other.
        if self.strategy.fills_after_done() {
            let settled = self.filled.max(filled);
            return Some(Transition::one(
                self.phase,
                settled,
                Applied::Filled(settled),
            ));
        }
        None
    }

    fn plan_filled(&self, filled: Shares) -> Result<Transition, ApplyError> {
        if filled != self.total {
            return Err(ApplyError::InconsistentFill {
                reported: filled,
                total: self.total,
            });
        }
        Ok(Transition::one(
            TrackedPhase::Done(Outcome::Filled),
            filled,
            Applied::Filled(filled),
        ))
    }

    fn plan_cancelled(&self, reported: Phase, filled: Shares) -> Transition {
        // A cancellation carrying the whole quantity means the fill won the race
        // against the client's cancel. The phase label is then stale against the
        // quantity in the same message, and the quantity decides.
        if filled == self.total {
            return Transition::one(
                TrackedPhase::Done(Outcome::Filled),
                filled,
                Applied::Filled(filled),
            );
        }
        let mut steps = Vec::new();
        if filled > self.filled {
            steps.push(Step {
                applied: Applied::Filled(filled),
                at: Snapshot::new(self.phase, filled),
            });
        }
        let phase = TrackedPhase::Done(Outcome::Cancelled);
        let settled = self.filled.max(filled);
        // A cancellation the client asked for is the expected reply to its own
        // cancel. One that arrives with no cancel outstanding came from IB or
        // the destination, and an operator has to tell the two apart.
        steps.push(Step {
            applied: if self.phase == TrackedPhase::PendingCancel {
                Applied::PhaseReport(reported)
            } else {
                Applied::CancelledByBroker { code: None }
            },
            at: Snapshot::new(phase, settled),
        });
        Transition {
            steps,
            phase,
            filled: settled,
        }
    }

    fn plan_live(&self, reported: Phase, filled: Shares) -> Transition {
        let advance = self.advance_fill(filled);
        if advance == Some(self.total) {
            return Transition::one(
                TrackedPhase::Done(Outcome::Filled),
                self.total,
                Applied::Filled(filled),
            );
        }
        let progressed = advance.unwrap_or(self.filled);
        let next = self.advance_phase(reported);
        let mut steps = Vec::new();
        if advance.is_some() {
            steps.push(Step {
                applied: Applied::Filled(filled),
                at: Snapshot::new(self.phase, progressed),
            });
        }
        if let Some(live) = next {
            steps.push(Step {
                applied: Applied::PhaseReport(live.as_phase()),
                at: Snapshot::new(TrackedPhase::Live(live), progressed),
            });
            return Transition {
                steps,
                phase: TrackedPhase::Live(live),
                filled: progressed,
            };
        }
        if !steps.is_empty() {
            return Transition {
                steps,
                phase: self.phase,
                filled: progressed,
            };
        }
        self.duplicate(reported)
    }

    fn duplicate(&self, reported: Phase) -> Transition {
        if self.strategy.overfills() {
            // The deviation counts the repeated snapshot as a new fill. It adds
            // one share and leaves the phase alone, so the order stays live with
            // a quantity past its size instead of the addition also closing it.
            let next = self
                .filled
                .checked_add(Shares::new(1))
                .unwrap_or(self.filled);
            return Transition::one(self.phase, next, Applied::Filled(next));
        }
        let phase = if self.strategy.rephases() {
            LivePhase::from_phase(reported).map_or(self.phase, TrackedPhase::Live)
        } else {
            self.phase
        };
        let at = Snapshot::new(phase, self.filled);
        Transition::one(phase, self.filled, Applied::Duplicate { at })
    }

    /// The cumulative fill after this report, or `None` when it adds nothing.
    ///
    /// Fills arrive as cumulative snapshots, so a redelivered or out-of-order
    /// snapshot must not move the number. Adding the reported value instead of
    /// taking the maximum counts the same shares once per delivery, which is
    /// what [`Strategy::DoubleCountsRedeliveredFill`] does.
    fn advance_fill(&self, reported: Shares) -> Option<Shares> {
        let next = if self.strategy.counts_deltas() {
            self.filled.checked_add(reported).unwrap_or(self.filled)
        } else {
            self.filled.max(reported)
        };
        if next > self.filled { Some(next) } else { None }
    }

    /// The live phase a report moves to, when the rules allow a move.
    ///
    /// A live report never leaves `PendingCancel`: the client's cancel is still
    /// outstanding, and IBKR ends that wait with a terminal status or an error,
    /// never with a status that means the cancel was forgotten. The fill such a
    /// report carries is still recorded.
    ///
    /// A report naming the phase the order already holds is not a move, so it
    /// contributes no entry. Otherwise a repeated snapshot would log a phase
    /// change it did not make.
    ///
    /// A report naming a phase the order already passed is not a move either,
    /// since [`LivePhase::advances_to`] refuses it. The reducer only reaches
    /// that refusal because a reconnect replays snapshots whose order the client
    /// cannot rely on, and following one would show a regression no broker
    /// reported.
    fn advance_phase(&self, reported: Phase) -> Option<LivePhase> {
        let next = LivePhase::from_phase(reported)?;
        match self.phase {
            TrackedPhase::PendingSubmit => Some(next),
            TrackedPhase::Live(current)
                if current != next
                    && (self.strategy.follows_stale() || current.advances_to(next)) =>
            {
                Some(next)
            }
            TrackedPhase::Live(_) | TrackedPhase::PendingCancel | TrackedPhase::Done(_) => None,
        }
    }

    fn plan_error(&self, code: i32) -> Transition {
        match code {
            // 201 means the order was rejected before it became active.
            201 if self.phase == TrackedPhase::PendingSubmit => Transition::one(
                TrackedPhase::Done(Outcome::Rejected),
                self.filled,
                Applied::Rejected { code },
            ),
            // 202 means IB cancelled an order that was already active.
            202 => Transition::one(
                TrackedPhase::Done(Outcome::Cancelled),
                self.filled,
                Applied::CancelledByBroker { code: Some(code) },
            ),
            _ => self.ignored(IgnoredReport::UnhandledError { code }),
        }
    }

    fn ignored(&self, reason: IgnoredReport) -> Transition {
        Transition::one(self.phase, self.filled, Applied::Ignored(reason))
    }

    fn commit(&mut self, transition: Transition) -> Result<&Applied, ApplyError> {
        if transition.steps.is_empty() || self.log.len() + transition.steps.len() > MAX_LOG {
            return Err(ApplyError::LogFull);
        }
        self.phase = transition.phase;
        self.filled = transition.filled;
        for step in transition.steps {
            self.log.push(step.applied);
            self.history.push(step.at);
        }
        self.log.last().ok_or(ApplyError::LogFull)
    }
}

/// One id's worth of order book.
///
/// A reused id is a protocol error the client must refuse before it reaches the
/// broker, so the slot is explicit rather than an `Option` a caller could forget
/// to check.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum OrderSlot {
    Free,
    Placed(Order),
}

impl OrderSlot {
    #[must_use]
    pub const fn order(&self) -> Option<&Order> {
        match self {
            Self::Free => None,
            Self::Placed(order) => Some(order),
        }
    }

    const fn order_mut(&mut self) -> Option<&mut Order> {
        match self {
            Self::Free => None,
            Self::Placed(order) => Some(order),
        }
    }
}

/// An empty book for `capacity` ids starting at id zero.
#[must_use]
pub fn slots(capacity: usize) -> Vec<OrderSlot> {
    vec![OrderSlot::Free; capacity]
}

/// # Errors
///
/// [`ApplyError::UnknownOrder`] when the id names no slot this client was
/// given, and [`ApplyError::DuplicateOrder`] when the slot already holds an
/// order.
pub fn place_order(
    orders: &mut [OrderSlot],
    id: OrderId,
    total: Shares,
    strategy: Strategy,
) -> Result<&Order, ApplyError> {
    let slot = slot_mut(orders, id).ok_or(ApplyError::UnknownOrder(Some(id)))?;
    match slot {
        OrderSlot::Free => {
            *slot = OrderSlot::Placed(Order::place(total).with_strategy(strategy));
            match slot {
                OrderSlot::Placed(order) => Ok(&*order),
                OrderSlot::Free => Err(ApplyError::DuplicateOrder(id)),
            }
        }
        OrderSlot::Placed(_) => Err(ApplyError::DuplicateOrder(id)),
    }
}

/// # Errors
///
/// [`ApplyError::UnknownOrder`] when the id names no placed order, plus what
/// [`Order::request_cancel`] returns.
pub fn request_cancel(orders: &mut [OrderSlot], id: OrderId) -> Result<&Applied, ApplyError> {
    let slot = slot_mut(orders, id).ok_or(ApplyError::UnknownOrder(Some(id)))?;
    slot.order_mut()
        .ok_or(ApplyError::UnknownOrder(Some(id)))?
        .request_cancel()
}

/// # Errors
///
/// [`ApplyError::UnknownOrder`] when the report names no placed order, plus what
/// [`Order::apply`] returns.
pub fn deliver<'a>(
    orders: &'a mut [OrderSlot],
    report: &Report,
) -> Result<&'a Applied, ApplyError> {
    let id = report.order_id().ok_or(ApplyError::UnknownOrder(None))?;
    let slot = slot_mut(orders, id).ok_or(ApplyError::UnknownOrder(Some(id)))?;
    slot.order_mut()
        .ok_or(ApplyError::UnknownOrder(Some(id)))?
        .apply(report)
}

fn slot_mut(orders: &mut [OrderSlot], id: OrderId) -> Option<&mut OrderSlot> {
    usize::try_from(id.get())
        .ok()
        .and_then(|index| orders.get_mut(index))
}
