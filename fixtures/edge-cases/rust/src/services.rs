use std::sync::{Arc, Mutex};

use shared_kit::round_cents;

use crate::errors::AppError;
use crate::models::{self, Order};
use crate::repo::{MemoryRepo, OrderRepo};

pub struct OrderService {
    repo: Box<dyn OrderRepo>,
    cache: Arc<Mutex<MemoryRepo>>,
}

/// Ships orders.
impl OrderService {
    pub fn new(repo: Box<dyn OrderRepo>) -> Self {
        Self { repo, cache: Arc::new(Mutex::new(MemoryRepo::default())) }
    }

    pub fn checkout(&mut self, id: u64) -> Result<f64, AppError> {
        let order = self.repo.find(id)?;
        if order.items.is_empty() {
            return Err(AppError::Invalid("empty order".into()));
        }
        self.cache.lock().unwrap().save(order.clone());
        let total = order.total();
        assert!(total >= 0.0, "{}", describe(&order));
        Ok(round_cents(total))
    }

    pub fn open(&mut self, id: u64) -> Order {
        let order = models::Order::new(id);
        self.repo.save(order.clone());
        Self::log_open(&order);
        order
    }

    fn log_open(order: &Order) {
        println!("opened {}", describe(order));
    }
}

pub fn describe(order: &Order) -> String {
    format!("#{} ({} items)", order.id, order.items.len())
}

pub fn api_base() -> String {
    std::env::var("SHOP_API_URL").unwrap_or_default()
}

pub async fn sync_remote(client: &reqwest::Client) {
    let _ = client.post("https://example.com/api/orders").send().await;
}

pub fn error_code(repo: &dyn OrderRepo) -> u16 {
    if let Err(e) = repo.find(1) {
        return e.code();
    }
    0
}

pub fn generic_total<R: OrderRepo>(repo: &R, id: u64) -> usize {
    let _ = repo.find(id);
    repo.count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Order {
        Order::new(7)
    }

    #[test]
    fn describes_orders() {
        assert_eq!(describe(&fixture()), "#7 (0 items)");
    }
}
