//! A Stateright model of the order lifecycle.
//!
//! The reducer under test is [`crate::tracker::Order`], the same type the unit
//! tests drive. The model only supplies what a live client cannot choose: the
//! order commands and reports are queued, delivery order is free, and TWS
//! re-sends every open order's status after a reconnect.
//!
//! Every rule is a `Property::always`, so each is a predicate on one state. The
//! history those predicates read lives in [`crate::tracker::Order`]'s log and the
//! per-entry history beside it. A rule about what happened before can only be a
//! state invariant if the state carries what happened, which is why both are
//! bounded fields of the state rather than a side channel.
//!
//! Each rule is narrowed to the one fact it owns, so that a mutant fails the
//! property named for it and not its neighbours. The fill rules own the
//! quantity, `single_outcome` owns the closing entries and the phases after
//! them, and the history rules own one log position each. Widening any of
//! them, for example by counting a fill that reaches the order size as a second
//! closing entry, makes one mutant fail three rules and proves none of them.

use stateright::{Model, Property};

use crate::id::OrderId;
use crate::report::{Phase, Report, Shares};
use crate::tracker::{
    Applied, OrderSlot, Strategy, TrackedPhase, deliver, place_order, request_cancel, slots,
};

/// Every modelled order is this many shares, so one partial fill and one full
/// fill are both reachable.
pub const TOTAL: u64 = 2;

pub const MAX_QUEUED_COMMANDS: usize = 2;

/// Reports the broker may have queued back to the client at once.
///
/// The bound has to hold the widest answer to each command in flight, because
/// the broker may answer both before the client reads either. An acceptance is
/// two snapshots and a firm-side cancellation is an error plus a status, so two
/// answers are four reports and a bound of three could never hold them together.
pub const MAX_QUEUED_REPORTS: usize = 4;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Command {
    Place { id: OrderId },
    Cancel { id: OrderId },
}

impl Command {
    const fn id(self) -> OrderId {
        match self {
            Self::Place { id } | Self::Cancel { id } => id,
        }
    }
}

/// One nondeterministic answer the broker may give a queued command.
///
/// The broker never sees the client's tracker, so it answers from its own view
/// and may confirm, transmit, fill, cancel, reject, park, or cancel from the
/// firm side. Which one happens is the nondeterminism this model explores.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Reply {
    Accept,
    /// The acceptance and the transmission as one answer.
    ///
    /// TWS answers a placement with a stream of snapshots, not one message, and
    /// the stream reaches the client in whatever order the socket and the
    /// reconnect replay produce. Returning both snapshots from one reply is what
    /// puts `PreSubmitted` and `Submitted` in the queue for the same order, so
    /// the guard that refuses a stale one has something to refuse.
    AcceptThenTransmit,
    Transmit,
    PartFill,
    Fill,
    CancelConfirmed,
    Reject,
    CancelByFirm,
    Hold,
}

impl Reply {
    const ALL: [Self; 9] = [
        Self::Accept,
        Self::AcceptThenTransmit,
        Self::Transmit,
        Self::PartFill,
        Self::Fill,
        Self::CancelConfirmed,
        Self::Reject,
        Self::CancelByFirm,
        Self::Hold,
    ];

    /// How many reports the reply returns. A firm-side cancellation returns its
    /// error and then the cancellation status, and either may be delivered
    /// first because delivery order is free.
    const fn width(self) -> usize {
        match self {
            Self::AcceptThenTransmit | Self::CancelByFirm => 2,
            Self::Accept
            | Self::Transmit
            | Self::PartFill
            | Self::Fill
            | Self::CancelConfirmed
            | Self::Reject
            | Self::Hold => 1,
        }
    }

    const fn action(self, index: usize) -> Action {
        match self {
            Self::Accept => Action::BrokerAccept { index },
            Self::AcceptThenTransmit => Action::BrokerAcceptThenTransmit { index },
            Self::Transmit => Action::BrokerTransmit { index },
            Self::PartFill => Action::BrokerPartFill { index },
            Self::Fill => Action::BrokerFill { index },
            Self::CancelConfirmed => Action::BrokerCancelConfirmed { index },
            Self::Reject => Action::BrokerReject { index },
            Self::CancelByFirm => Action::BrokerCancelByFirm { index },
            Self::Hold => Action::BrokerHold { index },
        }
    }

    fn reports(self, command: Command) -> Vec<Report> {
        let total = Shares::new(TOTAL);
        let id = command.id();
        let status = |phase, filled| Report::Status {
            order_id: id,
            phase,
            filled,
            remaining: total.saturating_sub(filled),
        };
        match self {
            Self::Accept => vec![status(Phase::PreSubmitted, Shares::ZERO)],
            Self::AcceptThenTransmit => vec![
                status(Phase::PreSubmitted, Shares::ZERO),
                status(Phase::Submitted, Shares::ZERO),
            ],
            Self::Transmit => vec![status(Phase::Submitted, Shares::ZERO)],
            Self::PartFill => vec![status(Phase::Submitted, Shares::new(TOTAL - 1))],
            Self::Fill => vec![status(Phase::Filled, total)],
            Self::CancelConfirmed => vec![status(Phase::Cancelled, Shares::ZERO)],
            Self::Reject => vec![Report::Error {
                order_id: Some(id),
                code: 201,
                message: "order rejected before it became active".into(),
            }],
            Self::CancelByFirm => vec![
                Report::Error {
                    order_id: Some(id),
                    code: 202,
                    message: "order cancelled by IB".into(),
                },
                status(Phase::Cancelled, Shares::ZERO),
            ],
            Self::Hold => vec![status(Phase::Inactive, Shares::ZERO)],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Action {
    Place { id: OrderId },
    Cancel { id: OrderId },
    BrokerAccept { index: usize },
    BrokerAcceptThenTransmit { index: usize },
    BrokerTransmit { index: usize },
    BrokerPartFill { index: usize },
    BrokerFill { index: usize },
    BrokerCancelConfirmed { index: usize },
    BrokerReject { index: usize },
    BrokerCancelByFirm { index: usize },
    BrokerHold { index: usize },
    Deliver { index: usize },
    Reconnect { id: OrderId },
}

/// Everything the model tracks. Each field is something a real client holds:
/// the ids TWS granted, the orders it placed, and the two queues of the
/// connection.
///
/// What the client recorded is not a separate field. The tracker's log is that
/// record, so a rule about history reads the log rather than a second copy that
/// could disagree with it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SystemState {
    pub next_valid_id: i32,
    pub orders: Vec<OrderSlot>,
    pub to_broker: Vec<Command>,
    pub to_client: Vec<Report>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Lifecycle {
    pub strategy: Strategy,
    pub orders: u8,
}

impl Lifecycle {
    #[must_use]
    pub const fn new(strategy: Strategy, orders: u8) -> Self {
        Self { strategy, orders }
    }

    fn ids(&self) -> impl Iterator<Item = OrderId> + '_ {
        (0..self.orders).map(|raw| OrderId::new(i32::from(raw)))
    }

    fn answer(state: &SystemState, index: usize, reply: Reply) -> Option<SystemState> {
        let command = *state.to_broker.get(index)?;
        let mut next = state.clone();
        next.to_broker.remove(index);
        next.to_client.extend(reply.reports(command));
        Some(next)
    }
}

fn order_of(state: &SystemState, id: OrderId) -> Option<&crate::tracker::Order> {
    let index = usize::try_from(id.get()).ok()?;
    state.orders.get(index)?.order()
}

fn all_orders(state: &SystemState) -> impl Iterator<Item = &crate::tracker::Order> {
    state.orders.iter().filter_map(OrderSlot::order)
}

impl Model for Lifecycle {
    type State = SystemState;
    type Action = Action;

    fn init_states(&self) -> Vec<Self::State> {
        vec![SystemState {
            next_valid_id: i32::from(self.orders),
            orders: slots(usize::from(self.orders)),
            to_broker: Vec::new(),
            to_client: Vec::new(),
        }]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        let command_room = state.to_broker.len() < MAX_QUEUED_COMMANDS;
        let report_room = state.to_client.len() < MAX_QUEUED_REPORTS;

        for id in self.ids() {
            if command_room && id.get() < state.next_valid_id && order_of(state, id).is_none() {
                actions.push(Action::Place { id });
            }
        }
        for id in self.ids() {
            if command_room && order_of(state, id).is_some() {
                actions.push(Action::Cancel { id });
            }
        }
        for index in 0..state.to_broker.len() {
            for reply in Reply::ALL {
                if state.to_client.len() + reply.width() <= MAX_QUEUED_REPORTS {
                    actions.push(reply.action(index));
                }
            }
        }
        for index in 0..state.to_client.len() {
            actions.push(Action::Deliver { index });
        }
        for id in self.ids() {
            if report_room && order_of(state, id).is_some() {
                actions.push(Action::Reconnect { id });
            }
        }
    }

    fn next_state(&self, state: &Self::State, action: Self::Action) -> Option<Self::State> {
        match action {
            Action::Place { id } => {
                let mut next = state.clone();
                place_order(&mut next.orders, id, Shares::new(TOTAL), self.strategy).ok()?;
                next.to_broker.push(Command::Place { id });
                Some(next)
            }
            Action::Cancel { id } => {
                let mut next = state.clone();
                request_cancel(&mut next.orders, id).ok()?;
                next.to_broker.push(Command::Cancel { id });
                Some(next)
            }
            Action::BrokerAccept { index } => Self::answer(state, index, Reply::Accept),
            Action::BrokerAcceptThenTransmit { index } => {
                Self::answer(state, index, Reply::AcceptThenTransmit)
            }
            Action::BrokerTransmit { index } => Self::answer(state, index, Reply::Transmit),
            Action::BrokerPartFill { index } => Self::answer(state, index, Reply::PartFill),
            Action::BrokerFill { index } => Self::answer(state, index, Reply::Fill),
            Action::BrokerCancelConfirmed { index } => {
                Self::answer(state, index, Reply::CancelConfirmed)
            }
            Action::BrokerReject { index } => Self::answer(state, index, Reply::Reject),
            Action::BrokerCancelByFirm { index } => Self::answer(state, index, Reply::CancelByFirm),
            Action::BrokerHold { index } => Self::answer(state, index, Reply::Hold),
            Action::Deliver { index } => {
                let mut next = state.clone();
                let report = next.to_client.get(index)?.clone();
                next.to_client.remove(index);
                // A report the tracker refuses is dropped instead of staying
                // queued. It cannot become acceptable later, and a queue that
                // held it would model a stall no client would sit in.
                let _refused = deliver(&mut next.orders, &report);
                Some(next)
            }
            Action::Reconnect { id } => {
                let (phase, filled, remaining) = order_of(state, id)?.snapshot()?;
                let mut next = state.clone();
                next.to_client.push(Report::Status {
                    order_id: id,
                    phase,
                    filled,
                    remaining,
                });
                Some(next)
            }
        }
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always("filled_within_total", filled_within_total),
            Property::always("reported_filled_is_monotonic", reported_filled_is_monotonic),
            Property::always("no_fill_after_done", no_fill_after_done),
            Property::always("single_outcome", single_outcome),
            Property::always(
                "duplicate_report_changes_nothing",
                duplicate_report_changes_nothing,
            ),
            Property::always("cancel_after_done_is_refused", cancel_after_done_is_refused),
            Property::always("phase_never_regresses", phase_never_regresses),
        ]
    }
}

fn filled_within_total(_model: &Lifecycle, state: &SystemState) -> bool {
    all_orders(state).all(|order| order.filled() <= order.total())
}

/// The tracker's fill equals the largest cumulative fill its log records.
///
/// The wire reports cumulative snapshots, so taking the maximum is what keeps
/// this true across a redelivered or out-of-order report. A reducer that adds
/// the reported value instead records entries that never reach the fill it
/// holds, which is the disagreement this rule reads.
fn reported_filled_is_monotonic(_model: &Lifecycle, state: &SystemState) -> bool {
    all_orders(state).all(|order| {
        let recorded = order
            .log()
            .iter()
            .filter_map(|entry| match entry {
                Applied::Filled(shares) => Some(*shares),
                _ => None,
            })
            .max();
        recorded.unwrap_or(Shares::ZERO) == order.filled()
    })
}

/// No order records a fill once its own history has closed it.
///
/// The history is the per-entry sequence of states, so this reads what the order
/// was at the time rather than inferring it from the current state.
fn no_fill_after_done(_model: &Lifecycle, state: &SystemState) -> bool {
    all_orders(state).all(|order| {
        order.history().iter().enumerate().all(|(index, before)| {
            !before.phase.is_done()
                || !matches!(order.log().get(index + 1), Some(Applied::Filled(_)))
        })
    })
}

/// An order closes at most once, and its history never re-opens it.
///
/// Two facts, because the two ways to break "one outcome" look different. A
/// second closing entry shows up in the count, and only the entries that state
/// an ending count: a fill that reaches the order size is a consequence of the
/// quantity, which [`filled_within_total`] and
/// [`reported_filled_is_monotonic`] own. A revival shows up in the history, since
/// a reducer that moves a finished order back to a live phase writes no second
/// closing entry at all.
fn single_outcome(_model: &Lifecycle, state: &SystemState) -> bool {
    all_orders(state).all(|order| {
        let closes = order
            .log()
            .iter()
            .filter(|entry| entry.broker_ended())
            .count();
        if closes > 1 {
            return false;
        }
        order.history().windows(2).all(|pair| match pair {
            [before, after] => !before.phase.is_done() || after.phase.is_done(),
            _ => true,
        })
    })
}

/// A report recorded as changing nothing found the state its own history
/// implies.
///
/// The spec offers two readings. "The current state is reachable without the
/// duplicate" is a claim about the whole log, so a reducer that miscounts a fill
/// also breaks it, and the rule would fire alongside the fill rule instead of
/// naming its own fault. "The duplicate is the last entry" only describes a
/// duplicate at the tail. The reading taken here stays a pure function of one
/// state and stays specific: each [`Applied::Duplicate`] carries the snapshot
/// the order held after the report, and that snapshot must equal the state the
/// history records just before the entry. A reducer that moves something on a
/// duplicate writes a snapshot its own history contradicts.
fn duplicate_report_changes_nothing(_model: &Lifecycle, state: &SystemState) -> bool {
    all_orders(state).all(|order| {
        order
            .log()
            .iter()
            .enumerate()
            .all(|(index, entry)| match entry {
                Applied::Duplicate { at } => *at == order.state_before(index),
                _ => true,
            })
    })
}

fn cancel_after_done_is_refused(_model: &Lifecycle, state: &SystemState) -> bool {
    all_orders(state).all(|order| {
        order.history().iter().enumerate().all(|(index, before)| {
            !before.phase.is_done()
                || !matches!(order.log().get(index + 1), Some(Applied::CancelRequested))
        })
    })
}

/// An order never goes back to a stage the broker already carried it past.
///
/// The broker answers with a stream of snapshots, and a reconnect replays them
/// in an order the client cannot rely on. Following a `PreSubmitted` that lands
/// after the `Submitted` it precedes would show the order rewinding, which is a
/// regression the broker never reported. [`Strategy::FollowsStalePhase`] is the
/// reducer that follows it.
///
/// [`crate::tracker::LivePhase::Inactive`] has no rank, because a hold and its
/// release move in both directions by design. A pair with an unranked side is
/// exempt.
///
/// The rule reads only the entries that record a phase move, because a reducer
/// that rewinds on a report its own entry calls a duplicate contradicts that
/// entry rather than the acceptance order, and [`duplicate_report_changes_nothing`]
/// owns it.
fn phase_never_regresses(_model: &Lifecycle, state: &SystemState) -> bool {
    all_orders(state).all(|order| {
        order.history().iter().enumerate().all(|(index, before)| {
            let Some(after) = order.history().get(index + 1) else {
                return true;
            };
            if !matches!(order.log().get(index + 1), Some(Applied::PhaseReport(_))) {
                return true;
            }
            match (before.phase, after.phase) {
                (TrackedPhase::Live(before), TrackedPhase::Live(after)) => {
                    match (before.rank(), after.rank()) {
                        (Some(before), Some(after)) => after >= before,
                        _ => true,
                    }
                }
                _ => true,
            }
        })
    })
}
