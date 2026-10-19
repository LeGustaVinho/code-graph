//! Language detection and Tree-sitter grammar binding.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use tree_sitter::{Parser, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    CSharp,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Prisma,
    Config,
}

impl Language {
    /// Canonical language label stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Language::CSharp => "csharp",
            Language::TypeScript | Language::Tsx => "typescript",
            Language::JavaScript => "javascript",
            Language::Python => "python",
            Language::Prisma => "prisma",
            Language::Config => "config",
        }
    }

    /// Languages that can reference each other's symbols directly (same runtime).
    pub fn family(self) -> &'static str {
        match self {
            Language::CSharp => "dotnet",
            Language::TypeScript | Language::Tsx | Language::JavaScript => "js",
            Language::Python => "python",
            Language::Prisma => "prisma",
            Language::Config => "config",
        }
    }

    pub fn from_path(path: &Path) -> Option<Language> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        if is_config_file_name(&name) {
            return Some(Language::Config);
        }
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "cs" => Some(Language::CSharp),
            "ts" | "mts" | "cts" => {
                if name.ends_with(".d.ts") {
                    None
                } else {
                    Some(Language::TypeScript)
                }
            }
            "tsx" => Some(Language::Tsx),
            "js" | "jsx" | "mjs" | "cjs" => {
                if name.ends_with(".min.js") {
                    None
                } else {
                    Some(Language::JavaScript)
                }
            }
            "py" | "pyi" => Some(Language::Python),
            "prisma" => Some(Language::Prisma),
            _ => None,
        }
    }

    pub fn has_tree_sitter(self) -> bool {
        !matches!(self, Language::Prisma | Language::Config)
    }

    pub fn ts_language(self) -> Option<tree_sitter::Language> {
        Some(match self {
            Language::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            _ => return None,
        })
    }
}

/// Configuration files whose keys are indexed for the config matrix.
pub fn is_config_file_name(lower_name: &str) -> bool {
    (lower_name.starts_with("appsettings") && lower_name.ends_with(".json"))
        || lower_name == ".env"
        || lower_name.starts_with(".env.")
        || lower_name == "local.settings.json"
}

thread_local! {
    static PARSERS: RefCell<HashMap<Language, Parser>> = RefCell::new(HashMap::new());
}

/// Parses `source` with a thread-local parser for `lang` (safe to call from Rayon workers).
pub fn parse(lang: Language, source: &str) -> Option<Tree> {
    let ts_lang = lang.ts_language()?;
    PARSERS.with(|cell| {
        let mut map = cell.borrow_mut();
        let parser = map.entry(lang).or_insert_with(|| {
            let mut p = Parser::new();
            p.set_language(&ts_lang).expect("incompatible tree-sitter grammar");
            p
        });
        parser.parse(source, None)
    })
}
