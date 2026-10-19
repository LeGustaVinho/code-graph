//! CodeGraph core: file discovery, Blake3 hashing, Tree-sitter parsing and purely
//! syntactic fact extraction for C#, TypeScript/TSX, JavaScript, Python, Prisma
//! schemas and configuration files.

pub mod diff;
pub mod extract;
pub mod hash;
pub mod lang;
pub mod model;
pub mod render;
pub mod routes;
pub mod tokens;
pub mod walk;

pub use extract::extract_file;

/// Bump whenever extraction output changes so cached indexes are rebuilt.
pub const EXTRACTOR_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+x4");
pub use lang::Language;
pub use model::*;
