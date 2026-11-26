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
    assert_eq!(MAX_QUEUED_REPORTS, 3);
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
