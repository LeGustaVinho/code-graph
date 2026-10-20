use std::collections::HashMap;

use super::OrderRepo;
use crate::errors::AppError;
use crate::models::Order;

#[derive(Default)]
pub struct MemoryRepo {
    orders: HashMap<u64, Order>,
}

impl OrderRepo for MemoryRepo {
    fn find(&self, id: u64) -> Result<Order, AppError> {
        self.orders.get(&id).cloned().ok_or(AppError::NotFound(id))
    }

    fn save(&mut self, order: Order) {
        self.orders.insert(order.id, order);
    }
}

impl MemoryRepo {
    pub fn with_seed(orders: Vec<Order>) -> Self {
        let mut repo = Self::default();
        for o in orders {
            repo.save(o);
        }
        repo
    }
}
