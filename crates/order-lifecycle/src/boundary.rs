use core::fmt;

use tws_api::proto;

use crate::id::OrderId;
use crate::report::{Phase, Report, Shares};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WireError {
    MissingOrderId,
    MissingStatus,
    /// A `filled` or `remaining` string that is not an unsigned integer.
    BadQuantity(String),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOrderId => f.write_str("message has no order id"),
            Self::MissingStatus => f.write_str("message has no order status"),
            Self::BadQuantity(quantity) => write!(f, "quantity is not a share count: {quantity:?}"),
        }
    }
}

impl core::error::Error for WireError {}

/// # Errors
///
/// A [`WireError`] when the message omits the order id or the status string,
/// or carries a quantity that is not an unsigned integer.
pub fn parse_status(msg: &proto::OrderStatus) -> Result<Report, WireError> {
    let order_id = msg
        .order_id
        .map(OrderId::new)
        .ok_or(WireError::MissingOrderId)?;
    let status = msg.status.as_deref().ok_or(WireError::MissingStatus)?;
    let filled = parse_quantity(msg.filled.as_deref())?;
    let remaining = parse_quantity(msg.remaining.as_deref())?;
    Ok(Report::Status {
        order_id,
        phase: Phase::from_wire(status),
        filled,
        remaining,
    })
}

/// Parses an error message. The 201/202 mapping belongs to the tracker, so a
/// rejection and a cancellation reach it as the same `Error` report.
#[must_use]
pub fn parse_error(msg: &proto::ErrorMessage) -> Report {
    let order_id = msg.id.map(OrderId::new);
    let code = msg.error_code.unwrap_or_default();
    Report::Error {
        order_id,
        code,
        message: msg.error_msg.clone().unwrap_or_default(),
    }
}

/// Builds the request the client transmits. The contract and order are passed
/// through untouched: this crate models the lifecycle, not the order itself.
#[must_use]
pub fn place_request(
    id: OrderId,
    contract: &proto::Contract,
    order: &proto::Order,
) -> proto::PlaceOrderRequest {
    proto::PlaceOrderRequest {
        order_id: Some(id.get()),
        contract: Some(contract.clone()).into(),
        order: Some(order.clone()).into(),
        ..proto::PlaceOrderRequest::default()
    }
}

#[must_use]
pub fn cancel_request(id: OrderId) -> proto::CancelOrderRequest {
    proto::CancelOrderRequest {
        order_id: Some(id.get()),
        ..proto::CancelOrderRequest::default()
    }
}

/// A missing field is zero shares: TWS omits the string rather than sending
/// `"0"`, and an order that has not started filling reports nothing.
fn parse_quantity(raw: Option<&str>) -> Result<Shares, WireError> {
    let Some(raw) = raw else {
        return Ok(Shares::ZERO);
    };
    raw.parse::<u64>()
        .map(Shares::new)
        .map_err(|_| WireError::BadQuantity(raw.to_owned()))
}
