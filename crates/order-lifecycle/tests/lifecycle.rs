// Test assertions report failure by panicking, which the workspace lint set
// bans in library code.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use order_lifecycle::boundary::{cancel_request, parse_error, parse_status, place_request};
use order_lifecycle::{
    Applied, ApplyError, IgnoredReport, LivePhase, Order, OrderId, Outcome, Phase, Report, Shares,
    TrackedPhase, WireError,
};
use tws_api::buffa::Message;
use tws_api::proto;

const ID: OrderId = OrderId::new(7);
const TOTAL: Shares = Shares::new(10);

const fn status(phase: Phase, filled: u64) -> Report {
    Report::Status {
        order_id: ID,
        phase,
        filled: Shares::new(filled),
        remaining: TOTAL.saturating_sub(Shares::new(filled)),
    }
}

fn order() -> Order {
    Order::place(TOTAL)
}

fn apply(order: &mut Order, report: &Report) -> Applied {
    order.apply(report).expect("report applies").clone()
}

#[test]
fn happy_path_walks_from_pending_submit_to_filled() {
    let mut order = order();
    assert_eq!(order.phase(), TrackedPhase::PendingSubmit);

    apply(&mut order, &status(Phase::PreSubmitted, 0));
    assert_eq!(order.phase(), TrackedPhase::Live(LivePhase::PreSubmitted));

    apply(&mut order, &status(Phase::Submitted, 0));
    assert_eq!(order.phase(), TrackedPhase::Live(LivePhase::Submitted));

    apply(&mut order, &status(Phase::Filled, 10));
    assert_eq!(order.phase(), TrackedPhase::Done(Outcome::Filled));
    assert_eq!(order.filled(), TOTAL);
}

#[test]
fn a_partial_fill_then_a_full_fill() {
    let mut order = order();
    apply(&mut order, &status(Phase::Submitted, 0));
    assert_eq!(
        apply(&mut order, &status(Phase::Submitted, 4)),
        Applied::Filled(Shares::new(4))
    );
    assert_eq!(order.filled(), Shares::new(4));
    assert_eq!(order.phase(), TrackedPhase::Live(LivePhase::Submitted));

    apply(&mut order, &status(Phase::Filled, 10));
    assert_eq!(order.filled(), TOTAL);
    assert_eq!(order.phase(), TrackedPhase::Done(Outcome::Filled));
}

#[test]
fn a_full_fill_carried_on_a_working_snapshot_closes_the_order() {
    // TWS can report the final quantity while the status still reads
    // `Submitted`, because the status field and the quantity update separately.
    let mut order = order();
    apply(&mut order, &status(Phase::Submitted, 10));
    assert_eq!(order.phase(), TrackedPhase::Done(Outcome::Filled));
}

#[test]
fn a_cancel_that_a_fill_beats_says_the_order_filled() {
    let mut order = order();
    apply(&mut order, &status(Phase::Submitted, 0));
    order.request_cancel().expect("cancel is legal while live");

    // The fill lands first and completes the order.
    apply(&mut order, &status(Phase::Submitted, 10));
    assert_eq!(order.phase(), TrackedPhase::Done(Outcome::Filled));
    assert_eq!(order.filled(), TOTAL);
}

#[test]
fn a_fill_that_a_cancel_beats_says_the_order_cancelled() {
    let mut order = order();
    apply(&mut order, &status(Phase::Submitted, 0));
    order.request_cancel().expect("cancel is legal while live");

    let applied = apply(&mut order, &status(Phase::Cancelled, 4));
    assert_eq!(applied, Applied::PhaseReport(Phase::Cancelled));
    assert_eq!(order.phase(), TrackedPhase::Done(Outcome::Cancelled));
    assert_eq!(order.filled(), Shares::new(4));
}

#[test]
fn a_cancel_confirmed_with_the_whole_quantity_is_a_fill() {
    // The phase label and the quantity disagree here. The quantity is what
    // actually happened, so the order is filled rather than cancelled.
    let mut order = order();
    apply(&mut order, &status(Phase::Submitted, 0));
    order.request_cancel().expect("cancel is legal while live");
    apply(&mut order, &status(Phase::Cancelled, 10));
    assert_eq!(order.phase(), TrackedPhase::Done(Outcome::Filled));
}

#[test]
fn a_redelivered_snapshot_changes_nothing() {
    let mut order = order();
    apply(&mut order, &status(Phase::Submitted, 4));
    let before = order.clone();

    let applied = apply(&mut order, &status(Phase::Submitted, 4));

    assert!(matches!(applied, Applied::Duplicate { .. }));
    assert_eq!(order.phase(), before.phase());
    assert_eq!(order.filled(), before.filled());
}

#[test]
fn a_stale_snapshot_arriving_after_a_newer_one_changes_nothing() {
    let mut order = order();
    apply(&mut order, &status(Phase::PreSubmitted, 0));
    apply(&mut order, &status(Phase::Submitted, 6));
    let before = order.clone();

    let applied = apply(&mut order, &status(Phase::PreSubmitted, 2));

    assert!(matches!(applied, Applied::Duplicate { .. }));
    assert_eq!(order.phase(), before.phase());
    assert_eq!(order.filled(), before.filled());
}

#[test]
fn a_hold_and_its_release_both_move_the_order() {
    let mut order = order();
    apply(&mut order, &status(Phase::Submitted, 0));
    apply(&mut order, &status(Phase::Inactive, 0));
    assert_eq!(order.phase(), TrackedPhase::Live(LivePhase::Inactive));

    apply(&mut order, &status(Phase::Submitted, 0));
    assert_eq!(order.phase(), TrackedPhase::Live(LivePhase::Submitted));
}

#[test]
fn error_201_rejects_a_pending_order() {
    let mut order = order();
    let applied = apply(
        &mut order,
        &Report::Error {
            order_id: Some(ID),
            code: 201,
            message: "rejected".into(),
        },
    );
    assert_eq!(applied, Applied::Rejected { code: 201 });
    assert_eq!(order.phase(), TrackedPhase::Done(Outcome::Rejected));
}

#[test]
fn error_201_on_a_live_order_is_recorded_and_ignored() {
    // A live order has already passed the point 201 describes.
    let mut order = order();
    apply(&mut order, &status(Phase::Submitted, 0));
    let applied = apply(
        &mut order,
        &Report::Error {
            order_id: Some(ID),
            code: 201,
            message: "rejected".into(),
        },
    );
    assert_eq!(
        applied,
        Applied::Ignored(IgnoredReport::UnhandledError { code: 201 })
    );
    assert_eq!(order.phase(), TrackedPhase::Live(LivePhase::Submitted));
}

#[test]
fn error_202_cancels_a_live_order() {
    let mut order = order();
    apply(&mut order, &status(Phase::Submitted, 0));
    let applied = apply(
        &mut order,
        &Report::Error {
            order_id: Some(ID),
            code: 202,
            message: "cancelled by IB".into(),
        },
    );
    assert_eq!(applied, Applied::CancelledByBroker { code: Some(202) });
    assert_eq!(order.phase(), TrackedPhase::Done(Outcome::Cancelled));
}

#[test]
fn a_cancel_after_the_outcome_is_refused_and_changes_nothing() {
    let mut order = order();
    apply(&mut order, &status(Phase::Filled, 10));
    let before = order.clone();

    assert_eq!(order.request_cancel(), Err(ApplyError::AlreadyDone));
    assert_eq!(order, before);
}

#[test]
fn a_second_cancel_while_one_is_outstanding_is_a_duplicate() {
    let mut order = order();
    apply(&mut order, &status(Phase::Submitted, 0));
    order.request_cancel().expect("cancel is legal while live");
    let before = order.clone();

    let applied = order.request_cancel().expect("a repeat cancel").clone();

    assert!(matches!(applied, Applied::Duplicate { .. }));
    assert_eq!(order.phase(), before.phase());
    assert_eq!(order.filled(), before.filled());
}

#[test]
fn a_report_for_an_unknown_order_is_refused() {
    let mut orders = order_lifecycle::tracker::slots(2);
    let report = status(Phase::Submitted, 0);
    assert_eq!(
        order_lifecycle::tracker::deliver(&mut orders, &report),
        Err(ApplyError::UnknownOrder(Some(ID)))
    );
}

#[test]
fn a_filled_report_short_of_the_total_is_refused() {
    let mut order = order();
    let before = order.clone();
    assert_eq!(
        order.apply(&status(Phase::Filled, 4)),
        Err(ApplyError::InconsistentFill {
            reported: Shares::new(4),
            total: TOTAL,
        })
    );
    assert_eq!(order, before);
}

#[test]
fn a_report_that_fills_the_log_is_refused_and_changes_nothing() {
    let mut order = order();
    apply(&mut order, &status(Phase::Submitted, 0));
    while order.log().len() < order_lifecycle::tracker::MAX_LOG {
        apply(&mut order, &status(Phase::Submitted, 0));
    }
    let before = order.clone();

    assert_eq!(
        order.apply(&status(Phase::Submitted, 0)),
        Err(ApplyError::LogFull)
    );
    assert_eq!(order, before);
}

#[test]
fn a_client_set_phase_from_the_wire_is_ignored() {
    let mut order = order();
    let applied = apply(&mut order, &status(Phase::PendingCancel, 0));
    assert_eq!(
        applied,
        Applied::Ignored(IgnoredReport::ClientSetPhase(Phase::PendingCancel))
    );
    assert_eq!(order.phase(), TrackedPhase::PendingSubmit);
}

#[test]
fn an_unknown_status_string_is_ignored() {
    let mut order = order();
    apply(&mut order, &status(Phase::Submitted, 0));
    let applied = apply(&mut order, &status(Phase::Unknown, 0));
    assert_eq!(applied, Applied::Ignored(IgnoredReport::UnknownPhase));
    assert_eq!(order.phase(), TrackedPhase::Live(LivePhase::Submitted));
}

#[test]
fn a_report_after_the_outcome_is_ignored() {
    let mut order = order();
    apply(&mut order, &status(Phase::Filled, 10));
    let before = order.clone();

    let applied = apply(&mut order, &status(Phase::Submitted, 0));

    assert_eq!(applied, Applied::Ignored(IgnoredReport::LateAfterDone));
    assert_eq!(order.phase(), before.phase());
    assert_eq!(order.filled(), before.filled());
}

#[test]
fn the_wire_status_strings_map_to_the_documented_phases() {
    let cases = [
        ("PendingSubmit", Phase::PendingSubmit),
        ("PendingCancel", Phase::PendingCancel),
        ("ApiPending", Phase::ApiPending),
        ("PreSubmitted", Phase::PreSubmitted),
        ("Submitted", Phase::Submitted),
        ("Cancelled", Phase::Cancelled),
        ("ApiCancelled", Phase::ApiCancelled),
        ("Filled", Phase::Filled),
        ("Inactive", Phase::Inactive),
        ("PreCancelled", Phase::Unknown),
        ("WarnState", Phase::Unknown),
        ("", Phase::Unknown),
    ];
    for (wire, expected) in cases {
        assert_eq!(Phase::from_wire(wire), expected, "wire value {wire:?}");
    }
}

#[test]
fn inactivity_is_terminal_by_neither_predicate() {
    // The client helper calls Inactive terminal; the callback documentation
    // calls it a pause. This model keeps it live, so both predicates are false.
    assert!(!Phase::Inactive.is_active());
    assert!(!Phase::Inactive.is_terminal());
    assert!(Phase::Submitted.is_active());
    assert!(Phase::Filled.is_terminal());
    assert!(Phase::ApiCancelled.is_terminal());
}

#[test]
fn quantities_reject_a_negative_or_fractional_value() {
    let negative = proto::OrderStatus::default()
        .with_order_id(7)
        .with_status("Submitted")
        .with_filled("-1");
    assert_eq!(
        parse_status(&negative),
        Err(WireError::BadQuantity("-1".into()))
    );

    let fractional = proto::OrderStatus::default()
        .with_order_id(7)
        .with_status("Submitted")
        .with_filled("1.5");
    assert_eq!(
        parse_status(&fractional),
        Err(WireError::BadQuantity("1.5".into()))
    );
}

#[test]
fn a_status_message_without_an_order_id_is_refused() {
    let message = proto::OrderStatus::default().with_status("Submitted");
    assert_eq!(parse_status(&message), Err(WireError::MissingOrderId));
}

#[test]
fn a_status_message_without_a_status_is_refused() {
    let message = proto::OrderStatus::default().with_order_id(7);
    assert_eq!(
        parse_status(&message),
        Err(WireError::MissingStatus(String::new()))
    );
}

#[test]
fn absent_quantities_parse_as_zero() {
    // TWS omits the quantity strings rather than sending "0".
    let message = proto::OrderStatus::default()
        .with_order_id(7)
        .with_status("Submitted");
    assert_eq!(
        parse_status(&message),
        Ok(Report::Status {
            order_id: ID,
            phase: Phase::Submitted,
            filled: Shares::ZERO,
            remaining: Shares::ZERO,
        })
    );
}

#[test]
fn an_error_without_an_order_id_parses_without_naming_one() {
    let message = proto::ErrorMessage::default()
        .with_error_code(1100)
        .with_error_msg("connectivity lost");
    assert_eq!(
        parse_error(&message),
        Ok(Report::Error {
            order_id: None,
            code: 1100,
            message: "connectivity lost".into(),
        })
    );
}

#[test]
fn an_error_carries_its_order_id_and_body() {
    let message = proto::ErrorMessage::default()
        .with_id(7)
        .with_error_code(201)
        .with_error_msg("order rejected");
    assert_eq!(
        parse_error(&message),
        Ok(Report::Error {
            order_id: Some(ID),
            code: 201,
            message: "order rejected".into(),
        })
    );
}

#[test]
fn a_status_survives_an_encode_and_decode() {
    // The generated types are the only wiring this crate has to the wire, so the
    // report a decoded message produces has to equal the report from the
    // in-memory one.
    let message = proto::OrderStatus::default()
        .with_order_id(7)
        .with_status("Submitted")
        .with_filled("3")
        .with_remaining("7");
    let decoded = proto::OrderStatus::decode_from_slice(&message.encode_to_vec())
        .expect("decoding an encoded OrderStatus");
    assert_eq!(decoded, message);
    assert_eq!(
        parse_status(&decoded),
        Ok(Report::Status {
            order_id: ID,
            phase: Phase::Submitted,
            filled: Shares::new(3),
            remaining: Shares::new(7),
        })
    );
}

#[test]
fn a_place_request_carries_the_id_and_the_order() {
    let contract = proto::Contract {
        symbol: Some("AAPL".into()),
        ..proto::Contract::default()
    };
    let order = proto::Order {
        action: Some("BUY".into()),
        total_quantity: Some("10".into()),
        ..proto::Order::default()
    };
    let request = place_request(ID, &contract, &order);
    assert_eq!(request.order_id, Some(7));
    assert_eq!(
        request.contract.as_option().map(|c| c.symbol.as_deref()),
        Some(Some("AAPL"))
    );
    assert_eq!(
        request.order.as_option().map(|o| o.action.as_deref()),
        Some(Some("BUY"))
    );
}

#[test]
fn a_cancel_request_carries_only_the_id() {
    let request = cancel_request(ID);
    assert_eq!(request.order_id, Some(7));
    assert!(request.order_cancel.as_option().is_none());
}
