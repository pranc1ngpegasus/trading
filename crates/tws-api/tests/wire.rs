// Test assertions report failure by panicking, which the workspace lint set
// bans in library code.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fmt::Write;

// Imported through the crate's re-export, so this file fails if `tws_api::buffa`
// stops resolving to the version the generated impls target.
use tws_api::buffa::Message;
use tws_api::proto::{Contract, Order, PlaceOrderRequest};

fn round_trip<M: Message>(message: &M) -> M {
    M::decode_from_slice(&message.encode_to_vec()).expect("re-encoding a decoded message")
}

fn encode_hex<M: Message>(message: &M) -> String {
    message
        .encode_to_vec()
        .iter()
        .fold(String::new(), |mut hex, byte| {
            write!(hex, "{byte:02x}").expect("writing to a String");
            hex
        })
}

#[test]
fn contract_round_trips_through_the_wire() {
    let contract = Contract {
        con_id: Some(265_598),
        symbol: Some("AAPL".into()),
        sec_type: Some("STK".into()),
        exchange: Some("SMART".into()),
        currency: Some("USD".into()),
        ..Contract::default()
    };
    assert_eq!(round_trip(&contract), contract);
}

#[test]
fn encoding_matches_protoc() {
    // Produced by `protoc --encode=protobuf.Contract Contract.proto`.
    let contract = Contract {
        con_id: Some(265_598),
        symbol: Some("AAPL".into()),
        sec_type: Some("STK".into()),
        exchange: Some("SMART".into()),
        currency: Some("USD".into()),
        ..Contract::default()
    };
    assert_eq!(
        encode_hex(&contract),
        "08fe9a1012044141504c1a0353544b4205534d4152545203555344"
    );
}

#[test]
fn absent_field_is_distinct_from_a_zero() {
    let unset = Contract::default();
    let zeroed = Contract {
        con_id: Some(0),
        ..Contract::default()
    };
    assert!(unset.con_id.is_none());
    assert_ne!(unset.encode_to_vec(), zeroed.encode_to_vec());
}

#[test]
fn place_order_request_nests_its_messages() {
    let request = PlaceOrderRequest {
        order_id: Some(7),
        contract: Some(Contract::default()).into(),
        order: Some(Order {
            action: Some("BUY".into()),
            total_quantity: Some("100".into()),
            ..Order::default()
        })
        .into(),
        ..PlaceOrderRequest::default()
    };
    let decoded = round_trip(&request);
    assert_eq!(decoded.order_id, Some(7));
    assert_eq!(
        decoded
            .order
            .as_option()
            .and_then(|order| order.action.as_deref()),
        Some("BUY")
    );
}

#[test]
fn version_comes_from_the_proto_drop() {
    assert_eq!(tws_api::API_VERSION, "10.50.02");
}
