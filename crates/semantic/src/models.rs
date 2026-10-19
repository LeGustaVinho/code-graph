//! Data model / ERD extraction (EF Core, Prisma, TypeORM, SQLAlchemy, Django, SQLModel, Pydantic).

use std::collections::{HashMap, HashSet};

use codegraph_core::extract::{clean_type, generic_args};
use codegraph_core::{Language, SymbolKind};

use crate::index::{Resolved, SymbolIndex};

#[derive(Clone, Debug)]
pub struct DataModel {
    pub id: String,
    pub name: String,
    pub framework: String,
    pub table: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ModelField {
    pub model_id: String,
    pub field_name: String,
    pub field_type: String,
    pub is_primary_key: bool,
    pub is_foreign_key: bool,
    pub target_model: Option<String>,
    pub cardinality: Option<String>,
    pub line: u32,
}

const COLLECTIONS: &[&str] = &[
    "ICollection",
    "IList",
    "List",
    "IEnumerable",
    "HashSet",
    "ISet",
    "Collection",
    "IReadOnlyCollection",
    "list",
    "List",
    "Set",
    "set",
    "Sequence",
];

fn detect(index: &SymbolIndex, i: usize, dbset_models: &HashSet<String>) -> Option<String> {
    let s = &index.syms[i];
    let sym = &s.sym;
    if !matches!(sym.kind, SymbolKind::Class | SymbolKind::DataModel | SymbolKind::Struct) {
        return None;
    }
    if sym.kind == SymbolKind::DataModel && sym.attrs.iter().any(|a| a.starts_with("prisma:")) {
        return Some("prisma".into());
    }
    match s.lang {
        Language::CSharp => {
            if dbset_models.contains(&sym.id) {
                return Some("ef-core".into());
            }
            if sym.attrs.iter().any(|a| a.starts_with("Table(")) {
                return Some("ef-core".into());
            }
        }
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            if sym.attrs.iter().any(|a| a == "Entity" || a.starts_with("Entity(")) {
                return Some("typeorm".into());
            }
            if sym.attrs.iter().any(|a| a.starts_with("Schema(")) {
                return Some("mongoose".into());
            }
        }
        Language::Python => {
            let bases: Vec<String> = sym.bases.iter().map(|b| b.trim().to_string()).collect();
            if bases.iter().any(|b| b == "models.Model" || b.ends_with(".models.Model")) {
                return Some("django".into());
            }
            if bases.iter().any(|b| b == "SQLModel") && sym.attrs.iter().any(|a| a.replace(' ', "") == "table=True") {
                return Some("sqlmodel".into());
            }
            let kids = index.children.get(&sym.id).cloned().unwrap_or_default();
            let sqla = kids.iter().any(|k| {
                let c = &index.syms[*k].sym;
                c.name == "__tablename__"
                    || c.value_text
                        .as_deref()
                        .map(|v| {
                            v.starts_with("Column(")
                                || v.starts_with("mapped_column(")
                                || v.starts_with("relationship(")
                                || v.starts_with("db.Column(")
                        })
                        .unwrap_or(false)
            });
            if sqla || bases.iter().any(|b| b == "db.Model") {
                return Some("sqlalchemy".into());
            }
            if bases.iter().any(|b| b == "Document" || b == "mongoengine.Document") {
                return Some("mongoengine".into());
            }
            if bases.iter().any(|b| b == "BaseModel" || b == "pydantic.BaseModel") {
                return Some("pydantic".into());
            }
        }
        _ => {}
    }
    None
}

/// Detects data models and their fields across all supported ORMs.
pub fn extract_models(index: &SymbolIndex) -> (Vec<DataModel>, Vec<ModelField>) {
    // EF Core: DbSet<T> properties mark T as an entity.
    let mut dbset_models = HashSet::new();
    for s in &index.syms {
        if s.lang != Language::CSharp || !matches!(s.sym.kind, SymbolKind::Property | SymbolKind::Field) {
            continue;
        }
        if let Some(t) = &s.sym.type_ref {
            if clean_type(t) == "DbSet" {
                if let Some(arg) = generic_args(t).into_iter().next() {
                    if let Resolved::One(m, _, _) = index.resolve_type(&arg, &s.file) {
                        dbset_models.insert(index.syms[m].sym.id.clone());
                    }
                }
            }
        }
    }
    let mut models = Vec::new();
    for i in 0..index.syms.len() {
        if let Some(fw) = detect(index, i, &dbset_models) {
            let s = &index.syms[i];
            let table = index
                .children
                .get(&s.sym.id)
                .and_then(|k| k.iter().find(|c| index.syms[**c].sym.name == "__tablename__"))
                .and_then(|c| index.syms[*c].sym.value_text.clone())
                .map(|v| v.trim_matches(|c| c == '"' || c == '\'').to_string());
            models.push(DataModel { id: s.sym.id.clone(), name: s.sym.name.clone(), framework: fw, table });
        }
    }
    let by_name: HashMap<String, &DataModel> = models.iter().map(|m| (m.name.clone(), m)).collect();
    let by_table: HashMap<String, &DataModel> = models.iter().filter_map(|m| m.table.clone().map(|t| (t, m))).collect();

    // First pass: raw fields with navigation targets.
    struct Raw {
        model: String,
        name: String,
        ty: String,
        pk: bool,
        fk_target: Option<String>,
        nav_target: Option<String>,
        collection: bool,
        explicit_card: Option<String>,
        line: u32,
    }
    let mut raws: Vec<Raw> = Vec::new();
    for m in &models {
        let Some(&mi) = index.by_id.get(&m.id) else { continue };
        let kids = index.children.get(&m.id).cloned().unwrap_or_default();
        let fw = m.framework.as_str();
        let mut relation_fk_fields: HashMap<String, String> = HashMap::new();
        if fw == "prisma" {
            for k in &kids {
                let c = &index.syms[*k].sym;
                for a in &c.attrs {
                    if let Some(rest) = a.strip_prefix("@relation(") {
                        if let Some(fs) = rest.find("fields:") {
                            let list = &rest[fs + 7..];
                            if let (Some(o), Some(e)) = (list.find('['), list.find(']')) {
                                for f in list[o + 1..e].split(',') {
                                    relation_fk_fields.insert(f.trim().to_string(), clean_type(c.type_ref.as_deref().unwrap_or("")));
                                }
                            }
                        }
                    }
                }
            }
        }
        for k in kids {
            let c = &index.syms[k].sym;
            if !matches!(c.kind, SymbolKind::Field | SymbolKind::Property) || c.name.starts_with("__") {
                continue;
            }
            if index.syms[mi].lang == Language::CSharp && c.is_static {
                continue;
            }
            let ty_raw = c.type_ref.clone().unwrap_or_default();
            let val = c.value_text.clone().unwrap_or_default();
            let mut r = Raw {
                model: m.id.clone(),
                name: c.name.clone(),
                ty: ty_raw.clone(),
                pk: false,
                fk_target: None,
                nav_target: None,
                collection: false,
                explicit_card: None,
                line: c.start_line,
            };
            match fw {
                "ef-core" => {
                    let base = clean_type(&ty_raw);
                    if COLLECTIONS.contains(&base.as_str()) {
                        if let Some(inner) = generic_args(&ty_raw).into_iter().next() {
                            let inner = clean_type(&inner);
                            if by_name.contains_key(&inner) {
                                r.nav_target = Some(inner);
                                r.collection = true;
                            }
                        }
                    } else if by_name.contains_key(&base) {
                        r.nav_target = Some(base);
                    }
                    r.pk = c.name == "Id" || c.name == format!("{}Id", m.name) || c.attrs.iter().any(|a| a == "Key");
                    if let Some(fk) = c.attrs.iter().find(|a| a.starts_with("ForeignKey(")) {
                        r.fk_target = fk.split('"').nth(1).map(|s| s.to_string());
                    }
                }
                "prisma" => {
                    let collection = ty_raw.ends_with("[]");
                    let base = clean_type(ty_raw.trim_end_matches("[]").trim_end_matches('?'));
                    if by_name.contains_key(&base) {
                        r.nav_target = Some(base);
                        r.collection = collection;
                    }
                    r.pk = c.attrs.iter().any(|a| a == "@id");
                    if let Some(t) = relation_fk_fields.get(&c.name) {
                        r.fk_target = Some(t.clone());
                    }
                }
                "typeorm" => {
                    for a in &c.attrs {
                        let an = a.split('(').next().unwrap_or(a);
                        let target = a
                            .split("=>")
                            .nth(1)
                            .map(|t| t.trim().trim_end_matches(')').trim().split([',', ')']).next().unwrap_or("").trim().to_string());
                        match an {
                            "PrimaryGeneratedColumn" | "PrimaryColumn" | "ObjectIdColumn" => r.pk = true,
                            "ManyToOne" => {
                                r.nav_target = target;
                                r.explicit_card = Some("N:1".into());
                            }
                            "OneToMany" => {
                                r.nav_target = target;
                                r.collection = true;
                                r.explicit_card = Some("1:N".into());
                            }
                            "ManyToMany" => {
                                r.nav_target = target;
                                r.collection = true;
                                r.explicit_card = Some("N:N".into());
                            }
                            "OneToOne" => {
                                r.nav_target = target;
                                r.explicit_card = Some("1:1".into());
                            }
                            _ => {}
                        }
                    }
                }
                "sqlalchemy" | "sqlmodel" => {
                    if val.starts_with("Column(")
                        || val.starts_with("db.Column(")
                        || val.starts_with("mapped_column(")
                        || val.starts_with("Field(")
                    {
                        let inner = val.split_once('(').map(|x| x.1).unwrap_or("");
                        let first = inner.split([',', ')']).next().unwrap_or("").trim().to_string();
                        if !first.starts_with("ForeignKey") && !first.contains('=') && !first.is_empty() {
                            r.ty = first;
                        }
                        r.pk = val.replace(' ', "").contains("primary_key=True");
                        if let Some(p) = val.find("ForeignKey(") {
                            let t = val[p + 11..].split(['"', '\'']).nth(1).unwrap_or("").to_string();
                            let table = t.split('.').next().unwrap_or("").to_string();
                            r.fk_target = Some(by_table.get(&table).map(|m| m.name.clone()).unwrap_or(table));
                        }
                    } else if val.starts_with("relationship(") || val.starts_with("db.relationship(") {
                        let target = val.split(['"', '\'']).nth(1).map(|s| s.to_string()).or_else(|| {
                            let inner = val.split_once('(').map(|x| x.1).unwrap_or("");
                            Some(inner.split([',', ')']).next().unwrap_or("").trim().to_string())
                        });
                        r.nav_target = target.filter(|t| !t.is_empty());
                        r.ty = "relationship".into();
                        let compact = val.replace(' ', "");
                        if compact.contains("uselist=False") {
                            r.explicit_card = Some("1:1".into());
                        } else if compact.contains("secondary=") {
                            r.collection = true;
                            r.explicit_card = Some("N:N".into());
                        }
                    }
                    if let Some(t) = &c.type_ref {
                        if t.starts_with("Mapped[") {
                            let inner = t.trim_start_matches("Mapped[").trim_end_matches(']');
                            r.ty = inner.to_string();
                            if inner.starts_with("List[") || inner.starts_with("list[") {
                                r.collection = true;
                            }
                        }
                    }
                }
                "django" => {
                    let field_kind = val.split('(').next().unwrap_or("").rsplit('.').next().unwrap_or("").to_string();
                    r.ty = field_kind.clone();
                    let target = val
                        .split_once('(')
                        .map(|x| x.1)
                        .unwrap_or("")
                        .split([',', ')'])
                        .next()
                        .unwrap_or("")
                        .trim()
                        .trim_matches(|c| c == '"' || c == '\'')
                        .to_string();
                    match field_kind.as_str() {
                        "ForeignKey" => {
                            r.fk_target = Some(target.clone());
                            r.nav_target = Some(target);
                            r.explicit_card = Some("N:1".into());
                        }
                        "OneToOneField" => {
                            r.fk_target = Some(target.clone());
                            r.nav_target = Some(target);
                            r.explicit_card = Some("1:1".into());
                        }
                        "ManyToManyField" => {
                            r.nav_target = Some(target);
                            r.collection = true;
                            r.explicit_card = Some("N:N".into());
                        }
                        _ => {}
                    }
                    r.pk = val.replace(' ', "").contains("primary_key=True");
                }
                _ => {
                    // pydantic / mongoose / mongoengine: typed attributes, nested models.
                    let collection = ty_raw.starts_with("List[") || ty_raw.starts_with("list[") || ty_raw.ends_with("[]");
                    let inner = if collection {
                        clean_type(
                            ty_raw.trim_start_matches("List[").trim_start_matches("list[").trim_end_matches(']').trim_end_matches("[]"),
                        )
                    } else {
                        clean_type(&ty_raw)
                    };
                    if by_name.contains_key(&inner) {
                        r.nav_target = Some(inner);
                        r.collection = collection;
                    }
                }
            }
            if r.ty.is_empty() {
                r.ty = val.split('(').next().unwrap_or("").to_string();
            }
            raws.push(r);
        }
    }
    // FK by convention: `CustomerId` next to a `Customer` navigation (EF / Prisma).
    let navs: HashMap<(String, String), String> =
        raws.iter().filter_map(|r| r.nav_target.clone().map(|t| ((r.model.clone(), r.name.to_ascii_lowercase()), t))).collect();
    for r in raws.iter_mut() {
        if r.fk_target.is_none() && r.nav_target.is_none() && !r.pk {
            let lower = r.name.to_ascii_lowercase();
            let stem = lower.strip_suffix("id").or_else(|| lower.strip_suffix("_id")).map(|s| s.trim_end_matches('_').to_string());
            if let Some(stem) = stem.filter(|s| !s.is_empty()) {
                if let Some(t) = navs.get(&(r.model.clone(), stem.clone())) {
                    r.fk_target = Some(t.clone());
                } else if let Some(m) = by_name.keys().filter(|k| k.to_ascii_lowercase() == stem).min() {
                    r.fk_target = Some(m.clone());
                }
            }
        }
    }
    // Cardinality from both sides of each navigation (scoped per ORM framework so that
    // e.g. a Prisma `Customer` and an EF `Customer` never see each other).
    let model_of: HashMap<String, (String, String)> =
        models.iter().map(|m| (m.id.clone(), (m.framework.clone(), m.name.clone()))).collect();
    let key_of = |r: &Raw| model_of.get(&r.model).cloned().unwrap_or_default();
    let back: HashMap<(String, String, String), bool> = raws
        .iter()
        .filter_map(|r| {
            let (fw, name) = key_of(r);
            r.nav_target.clone().map(|t| ((fw, name, t), r.collection))
        })
        .collect();
    let fk_to: HashSet<(String, String, String)> = raws
        .iter()
        .filter_map(|r| {
            let (fw, name) = key_of(r);
            r.fk_target.clone().map(|t| (fw, name, t))
        })
        .collect();
    let mut fields = Vec::new();
    for r in raws {
        let (fw, this) = key_of(&r);
        let card = match (&r.explicit_card, &r.nav_target) {
            (Some(c), _) => Some(c.clone()),
            (None, Some(t)) => {
                let other_side = back.get(&(fw.clone(), t.clone(), this.clone())).copied();
                let this_fk = fk_to.contains(&(fw.clone(), this.clone(), t.clone()));
                let other_fk = fk_to.contains(&(fw.clone(), t.clone(), this.clone()));
                Some(
                    match (r.collection, other_side) {
                        (true, Some(true)) => "N:N",
                        (true, _) => "1:N",
                        (false, Some(true)) => "N:1",
                        (false, _) if this_fk => "N:1",
                        (false, _) if other_fk && r.ty == "relationship" => "1:N",
                        (false, Some(false)) => "1:1",
                        (false, None) => "N:1",
                    }
                    .to_string(),
                )
            }
            (None, None) if r.fk_target.is_some() => Some("N:1".into()),
            _ => None,
        };
        fields.push(ModelField {
            model_id: r.model,
            field_name: r.name,
            field_type: r.ty,
            is_primary_key: r.pk,
            is_foreign_key: r.fk_target.is_some(),
            target_model: r.fk_target.or(r.nav_target),
            cardinality: card,
            line: r.line,
        });
    }
    (models, fields)
}
