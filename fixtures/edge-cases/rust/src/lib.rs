//! Shop API: orders, repositories and HTTP routes.

pub mod api;
pub mod db;
pub mod errors;
pub mod models;
pub mod repo;
pub mod services;

pub use models::Order;

pub const MAX_ITEMS: usize = 50;

pub mod util {
    pub mod fmt_helpers;

    pub fn shout(s: &str) -> String {
        self::fmt_helpers::bang(s)
    }
}

pub fn announce() -> String {
    util::shout("hi")
}

static mut REQUESTS: u64 = 0;

pub fn count_request() {
    unsafe {
        REQUESTS += 1;
    }
}

#[macro_export]
macro_rules! audit {
    ($e:expr) => {{
        $crate::count_request();
        $e
    }};
}
