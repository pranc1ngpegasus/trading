/// The API client's order id, as carried by `PlaceOrderRequest.order_id` and
/// `OrderStatus.order_id`.
///
/// A newtype so an order id can never stand in where a perm id or a share count
/// belongs. TWS assigns ids from the `NextValidId` message and expects the client
/// to use them in increasing order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OrderId(i32);

impl OrderId {
    #[must_use]
    pub const fn new(raw: i32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }
}

impl From<OrderId> for i32 {
    fn from(id: OrderId) -> Self {
        id.0
    }
}
