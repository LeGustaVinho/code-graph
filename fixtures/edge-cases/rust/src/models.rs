use serde::Serialize;

/// A customer order.
#[derive(Debug, Clone, Serialize)]
pub struct Order {
    pub id: u64,
    pub items: Vec<Item>,
    status: Status,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item(pub String, pub f64);

pub struct Token(u32);

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum Status {
    Open,
    Paid { at: u64 },
    Cancelled = 9,
}

impl Order {
    pub fn new(id: u64) -> Self {
        Self { id, items: Vec::new(), status: Status::Open }
    }

    pub fn code(&self) -> u16 {
        200
    }

    pub fn total(&self) -> f64 {
        self.items.iter().map(|i| i.1).sum()
    }
}
