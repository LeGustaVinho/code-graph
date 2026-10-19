//! Configuration files (appsettings*.json, .env*) → config key definitions.

use super::*;

pub(crate) fn extract(ctx: &mut Ctx) {
    let name = ctx.path.rsplit('/').next().unwrap_or(ctx.path).to_ascii_lowercase();
    if name.ends_with(".json") {
        json_keys(ctx);
    } else {
        env_keys(ctx);
    }
}

fn define(ctx: &mut Ctx, key: &str, source: &str, line: u32, value: Option<String>) {
    let module = ctx.module_id();
    ctx.facts.config.push(ConfigDep {
        key: key.to_string(),
        source_type: source.to_string(),
        line,
        default_value: value.map(|v| truncate(&v, 120)),
        usage: "define".into(),
        enclosing: Some(module),
    });
}

fn env_keys(ctx: &mut Ctx) {
    let lines: Vec<(u32, String)> = ctx.src.lines().enumerate().map(|(i, l)| (i as u32 + 1, l.to_string())).collect();
    for (lno, l) in lines {
        let t = l.trim().trim_start_matches("export ").trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            let k = k.trim();
            if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
                define(ctx, k, ".env", lno, Some(v));
            }
        }
    }
}

fn json_keys(ctx: &mut Ctx) {
    // Strip // comments (appsettings allows them) before parsing.
    let cleaned: String = ctx
        .src
        .lines()
        .map(|l| {
            let t = l.trim_start();
            if t.starts_with("//") {
                ""
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&cleaned) else { return };
    let mut keys: Vec<(String, Option<String>)> = Vec::new();
    flatten(&value, String::new(), &mut keys);
    // Approximate line numbers: search each leaf key name in order.
    let lines: Vec<&str> = ctx.src.lines().collect();
    let mut cursor = 0usize;
    let mut out = Vec::new();
    for (k, v) in keys {
        let leaf = k.rsplit(':').next().unwrap_or(&k).to_string();
        let needle = format!("\"{leaf}\"");
        let mut found = None;
        for (i, l) in lines.iter().enumerate().skip(cursor) {
            if l.contains(&needle) {
                found = Some(i);
                break;
            }
        }
        let lno = match found {
            Some(i) => {
                cursor = i + 1;
                i as u32 + 1
            }
            None => 1,
        };
        out.push((k, lno, v));
    }
    for (k, lno, v) in out {
        define(ctx, &k, "appsettings", lno, v);
    }
}

fn flatten(v: &serde_json::Value, prefix: String, out: &mut Vec<(String, Option<String>)>) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, child) in map {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}:{k}") };
                flatten(child, key, out);
            }
        }
        serde_json::Value::Array(_) => out.push((prefix, Some(v.to_string()))),
        serde_json::Value::String(s) => out.push((prefix, Some(s.clone()))),
        other => out.push((prefix, Some(other.to_string()))),
    }
}
