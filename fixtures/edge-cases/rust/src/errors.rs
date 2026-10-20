use std::fmt;

#[derive(Debug)]
pub enum AppError {
    NotFound(u64),
    Invalid(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl AppError {
    pub fn code(&self) -> u16 {
        404
    }
}
