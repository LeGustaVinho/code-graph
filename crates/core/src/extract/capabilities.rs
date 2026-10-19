//! Side-effect capability tags inferred from a symbol's own syntactic facts.

use std::collections::{BTreeSet, HashMap};

use super::Ctx;
use crate::model::RefKind;

const DB_READ_CALLS: &[&str] = &[
    "ToList",
    "ToListAsync",
    "ToArray",
    "ToArrayAsync",
    "FirstOrDefault",
    "FirstOrDefaultAsync",
    "SingleOrDefault",
    "SingleOrDefaultAsync",
    "First",
    "FirstAsync",
    "Find",
    "FindAsync",
    "AnyAsync",
    "CountAsync",
    "Include",
    "AsNoTracking",
    "FromSqlRaw",
    "findMany",
    "findUnique",
    "findFirst",
    "findOne",
    "findById",
    "findAll",
    "find",
    "findOneBy",
    "findBy",
    "aggregate",
    "query",
    "filter",
    "filter_by",
    "all",
    "first",
    "one",
    "scalar",
    "scalars",
    "get_or_404",
    "select",
    "fetchone",
    "fetchall",
    "QueryAsync",
    "QueryFirstOrDefaultAsync",
];

const DB_WRITE_CALLS: &[&str] = &[
    "SaveChanges",
    "SaveChangesAsync",
    "ExecuteSqlRaw",
    "ExecuteSqlRawAsync",
    "ExecuteAsync",
    "BulkInsert",
    "commit",
    "flush",
    "bulk_create",
    "bulk_update",
];

const NETWORK_CALLS: &[&str] = &[
    "fetch",
    "GetAsync",
    "PostAsync",
    "PutAsync",
    "DeleteAsync",
    "PatchAsync",
    "SendAsync",
    "GetStringAsync",
    "GetFromJsonAsync",
    "PostAsJsonAsync",
    "PutAsJsonAsync",
    "urlopen",
    "request",
];

const NETWORK_RECEIVERS: &[&str] = &["axios", "requests", "httpx", "http", "httpclient", "client", "ky", "superagent", "aiohttp", "urllib"];

const FILE_CALLS: &[&str] = &[
    "ReadAllText",
    "ReadAllTextAsync",
    "WriteAllText",
    "WriteAllTextAsync",
    "ReadAllBytes",
    "WriteAllBytes",
    "AppendAllText",
    "readFile",
    "readFileSync",
    "writeFile",
    "writeFileSync",
    "appendFile",
    "createReadStream",
    "createWriteStream",
    "unlink",
    "unlinkSync",
    "mkdir",
    "open",
    "read_text",
    "write_text",
    "read_bytes",
    "write_bytes",
];

const AUTH_MARKERS: &[&str] = &[
    "Authorize",
    "UseGuards",
    "AuthGuard",
    "login_required",
    "jwt_required",
    "requires_auth",
    "auth_required",
    "permission_required",
    "Roles",
    "RequireAuthorization",
    "permission_classes",
    "IsAuthenticated",
    "get_current_user",
    "current_user",
    "Security(",
];

pub(super) fn infer(ctx: &mut Ctx) {
    let facts = &ctx.facts;
    let mut caps: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut add = |id: &str, tag: &str| {
        caps.entry(id.to_string()).or_default().insert(tag.to_string());
    };

    for r in &facts.refs {
        let recv = r.receiver.as_deref().unwrap_or("").to_ascii_lowercase();
        let recv_last = recv.rsplit('.').next().unwrap_or("").trim_start_matches('_').to_string();
        let rtype = r.receiver_type.as_deref().unwrap_or("").to_ascii_lowercase();
        match r.kind {
            RefKind::Throw => add(&r.enclosing, "THROWS"),
            RefKind::Call => {
                let n = r.name.as_str();
                let dbish = recv.contains("db")
                    || recv.contains("context")
                    || recv.contains("ctx")
                    || recv.contains("prisma")
                    || recv.contains("repo")
                    || recv.contains("session")
                    || recv.contains("objects")
                    || recv.contains("collection")
                    || recv.contains("query")
                    || rtype.contains("dbcontext")
                    || rtype.contains("repository")
                    || rtype.contains("dbset")
                    || rtype.contains("session");
                if DB_READ_CALLS.contains(&n) && dbish {
                    add(&r.enclosing, "DB_READ");
                }
                if DB_WRITE_CALLS.contains(&n) && (dbish || n.starts_with("SaveChanges")) {
                    add(&r.enclosing, "DB_WRITE");
                }
                if NETWORK_CALLS.contains(&n)
                    && (r.receiver.is_none() || NETWORK_RECEIVERS.iter().any(|x| recv_last.contains(x)) || rtype.contains("httpclient"))
                {
                    add(&r.enclosing, "NETWORK_IO");
                }
                if NETWORK_RECEIVERS.iter().any(|x| recv_last == *x)
                    && matches!(n, "get" | "post" | "put" | "delete" | "patch" | "head" | "request")
                {
                    add(&r.enclosing, "NETWORK_IO");
                }
                let fileish = recv_last == "file"
                    || recv_last == "fs"
                    || recv_last == "directory"
                    || recv_last == "path"
                    || recv_last == "promises"
                    || (r.receiver.is_none() && n == "open");
                if FILE_CALLS.contains(&n) && fileish {
                    add(&r.enclosing, "FILE_IO");
                }
                if recv_last == "playerprefs" || recv_last == "editorprefs" {
                    add(&r.enclosing, "LOCAL_STORAGE");
                }
                if recv.contains("unitywebrequest")
                    || rtype.contains("unitywebrequest")
                    || n == "SendWebRequest"
                    || n == "GetHttpsCallable"
                    || (n == "CallAsync" && (recv.contains("callable") || recv.contains("function")))
                {
                    add(&r.enclosing, "NETWORK_IO");
                }
                if recv.contains("cache") || recv.contains("redis") || rtype.contains("cache") {
                    add(&r.enclosing, "CACHE");
                }
                if recv.contains("logger") || recv_last == "log" || recv_last == "console" {
                    add(&r.enclosing, "LOGGING");
                }
                if matches!(n, "publish" | "emit" | "Publish" | "PublishAsync" | "send_task" | "delay" | "dispatch") {
                    add(&r.enclosing, "EVENT_EMIT");
                }
            }
            _ => {}
        }
    }
    for m in &facts.mutations {
        match m.mutation_type.as_str() {
            "database" => add(&m.enclosing, "DB_WRITE"),
            "cache" => add(&m.enclosing, "CACHE"),
            "global_var" => add(&m.enclosing, "GLOBAL_STATE"),
            "storage" => add(&m.enclosing, "LOCAL_STORAGE"),
            _ => {}
        }
    }
    for h in &facts.http_calls {
        add(&h.enclosing, "NETWORK_IO");
    }
    for c in facts.config.iter().filter(|c| c.usage == "read") {
        if let Some(e) = &c.enclosing {
            add(e, "CONFIG_READ");
        }
    }

    // Authorization (own attributes or inherited from the enclosing class).
    let by_id: HashMap<String, (Vec<String>, Option<String>)> =
        facts.symbols.iter().map(|s| (s.id.clone(), (s.attrs.clone(), s.parent_id.clone()))).collect();
    let has_auth = |attrs: &[String]| -> Option<bool> {
        if attrs.iter().any(|a| a.starts_with("AllowAnonymous") || a.starts_with("Public(")) {
            return Some(false);
        }
        if attrs.iter().any(|a| AUTH_MARKERS.iter().any(|m| a.contains(m))) {
            return Some(true);
        }
        None
    };
    let mut auth_ids = Vec::new();
    for s in &facts.symbols {
        if !s.kind.is_callable() {
            continue;
        }
        let mut decision = has_auth(&s.attrs);
        if decision.is_none() {
            if let Some((pattrs, _)) = s.parent_id.as_ref().and_then(|p| by_id.get(p)) {
                decision = has_auth(pattrs);
            }
        }
        if decision.is_none() {
            if let Some(p) = &s.params {
                if p.list
                    .iter()
                    .any(|x| x.ty.as_deref().map(|t| t.contains("current_user") || t.contains("get_current_user")).unwrap_or(false))
                {
                    decision = Some(true);
                }
            }
        }
        if decision == Some(true) {
            auth_ids.push(s.id.clone());
        }
    }
    for id in auth_ids {
        add(&id, "AUTH_REQUIRED");
    }

    for s in ctx.facts.symbols.iter_mut() {
        if let Some(c) = caps.remove(&s.id) {
            s.capabilities.extend(c);
        }
    }
}
