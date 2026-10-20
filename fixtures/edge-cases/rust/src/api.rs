use axum::routing::{get, post};
use axum::Router;

use crate::services::describe;
use crate::Order;

pub fn router() -> Router {
    Router::new().route("/orders", get(list_orders).post(create_order)).route("/orders/:id", get(show_order))
}

async fn list_orders() -> String {
    String::new()
}

async fn create_order() -> String {
    crate::count_request();
    describe(&Order::new(1))
}

async fn show_order() -> String {
    String::new()
}
