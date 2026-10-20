mod memory;

pub use self::memory::MemoryRepo;

use crate::errors::AppError;
use crate::models::Order;

/// Order storage.
pub trait OrderRepo: Send + Sync {
    fn find(&self, id: u64) -> Result<Order, AppError>;
    fn save(&mut self, order: Order);
    fn count(&self) -> usize {
        0
    }
}
