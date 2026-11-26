// Test assertions report failure by panicking, which the workspace lint set
// bans in library code.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::time::{Duration, Instant};

use order_lifecycle::Strategy;
use order_lifecycle::checker::{Lifecycle, MAX_QUEUED_COMMANDS, MAX_QUEUED_REPORTS, TOTAL};
use order_lifecycle::tracker::MAX_LOG;
use stateright::{Checker, Model};

/// One order id. Every rule relates an order to its own history, never to
/// another order, so a second id explores the same rules against a larger
/// product: two ids take minutes and add no reachable violation that one id
/// misses.
const ORDERS: u8 = 1;

/// Every run finishes far inside this, so a rule change that blows the search up
/// fails the test instead of hanging CI.
const BUDGET: Duration = Duration::from_secs(40);

struct Run {
    states: usize,
    unique: usize,
    broken: Vec<&'static str>,
    wall: Duration,
}

fn explore(strategy: Strategy) -> Run {
    let started = Instant::now();
    let model = Lifecycle::new(strategy, ORDERS);
    let checker = model.checker().threads(2).spawn_bfs().join();
    let wall = started.elapsed();
    let mut broken: Vec<&'static str> = checker.discoveries().keys().copied().collect();
    broken.sort_unstable();
    Run {
        states: checker.state_count(),
        unique: checker.unique_state_count(),
        broken,
        wall,
    }
}

#[test]
fn correct_exhausts_with_no_discovery() {
    let run = explore(Strategy::Correct);
    println!(
        "Correct: {} states, {} unique, {} order id(s), log bound {}, \
         {} commands and {} reports queued, {:?} wall",
        run.states, run.unique, ORDERS, MAX_LOG, MAX_QUEUED_COMMANDS, MAX_QUEUED_REPORTS, run.wall
    );
    assert_eq!(run.broken, Vec::<&'static str>::new());
    assert!(run.states > 0);
    assert!(run.wall < BUDGET, "the exhaustive run took {:?}", run.wall);
}

#[test]
fn the_bounds_let_a_partial_fill_and_a_full_fill_both_happen() {
    // The bounds are only meaningful if a partial fill leaves a share for the
    // full one to take, and the log can hold a whole lifecycle.
    assert_eq!(TOTAL, 2);
    assert_eq!(MAX_QUEUED_COMMANDS, 2);
    assert_eq!(MAX_QUEUED_REPORTS, 4);
    const { assert!(MAX_LOG >= 5, "the log has to hold a full lifecycle run") };
}

#[test]
fn double_counting_a_redelivered_fill_breaks_monotonicity() {
    let run = explore(Strategy::DoubleCountsRedeliveredFill);
    assert_eq!(run.broken, vec!["reported_filled_is_monotonic"]);
    assert!(run.wall < BUDGET, "{:?}", run.wall);
}

#[test]
fn reviving_a_finished_order_breaks_the_single_outcome() {
    let run = explore(Strategy::RevivesAfterDone);
    assert_eq!(run.broken, vec!["single_outcome"]);
    assert!(run.wall < BUDGET, "{:?}", run.wall);
}

#[test]
fn accepting_a_second_outcome_breaks_the_single_outcome() {
    let run = explore(Strategy::AcceptsSecondOutcome);
    assert_eq!(run.broken, vec!["single_outcome"]);
    assert!(run.wall < BUDGET, "{:?}", run.wall);
}

#[test]
fn overfilling_on_a_duplicate_breaks_the_total() {
    let run = explore(Strategy::OverfillsOnDuplicate);
    assert_eq!(run.broken, vec!["filled_within_total"]);
    assert!(run.wall < BUDGET, "{:?}", run.wall);
}

#[test]
fn rephasing_on_a_duplicate_breaks_the_duplicate_rule() {
    let run = explore(Strategy::RephasesOnDuplicate);
    assert_eq!(run.broken, vec!["duplicate_report_changes_nothing"]);
    assert!(run.wall < BUDGET, "{:?}", run.wall);
}

#[test]
fn cancelling_a_finished_order_breaks_the_cancel_rule() {
    let run = explore(Strategy::CancelsAfterDone);
    assert_eq!(run.broken, vec!["cancel_after_done_is_refused"]);
    assert!(run.wall < BUDGET, "{:?}", run.wall);
}

#[test]
fn filling_a_finished_order_breaks_the_no_fill_rule() {
    // The spec lists `no_fill_after_done` as the rule `RevivesAfterDone` may
    // break, and that mutant does break `single_outcome`. This variant exists so
    // the no-fill rule has a reducer that can fail it on its own: an invariant
    // no mutation can break proves nothing.
    let run = explore(Strategy::FillsAfterDone);
    assert_eq!(run.broken, vec!["no_fill_after_done"]);
    assert!(run.wall < BUDGET, "{:?}", run.wall);
}

#[test]
fn following_a_stale_phase_breaks_the_acceptance_order() {
    // Only an acceptance that arrives as a stream puts two live phases in the
    // queue for one order, so without the two-snapshot reply this mutant has no
    // stale snapshot to follow and the guard is untested.
    let run = explore(Strategy::FollowsStalePhase);
    assert_eq!(run.broken, vec!["phase_never_regresses"]);
    assert!(run.wall < BUDGET, "{:?}", run.wall);
}
