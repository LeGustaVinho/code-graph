//! Prisma schema extractor (line based; Prisma has no Tree-sitter grammar in this build).

use super::*;

pub(crate) fn extract(ctx: &mut Ctx) {
    let src = ctx.src;
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() >= 2
            && (words[0] == "model" || words[0] == "enum" || words[0] == "type" || words[0] == "view")
            && line.ends_with('{')
        {
            let name = words[1].trim_end_matches('{').to_string();
            let start = i as u32 + 1;
            let mut end = start;
            let mut j = i + 1;
            while j < lines.len() {
                if lines[j].trim().starts_with('}') {
                    end = j as u32 + 1;
                    break;
                }
                j += 1;
            }
            let kind = if words[0] == "enum" { SymbolKind::Enum } else { SymbolKind::DataModel };
            let mut s = NewSymbol::new(kind, &name, ctx.child_qname(&name), start, end);
            s.signature = format!("{} {}", words[0], name);
            s.attrs = vec![format!("prisma:{}", words[0])];
            let block: String = lines[i..=(j.min(lines.len() - 1))].join("\n");
            s.body_hash = crate::hash::short_hash(&collapse_ws(&block));
            let id = ctx.add_symbol(s);
            ctx.push_scope(Scope { symbol_id: id, kind: Some(kind), qname: ctx.child_qname(&name), ..Default::default() });
            for (k, l) in lines.iter().enumerate().take(j).skip(i + 1) {
                let t = l.trim();
                if t.is_empty() || t.starts_with("//") || t.starts_with("@@") {
                    continue;
                }
                let parts: Vec<&str> = t.split_whitespace().collect();
                let fname = parts[0].to_string();
                let lno = k as u32 + 1;
                if kind == SymbolKind::Enum {
                    let mut c = NewSymbol::new(SymbolKind::Const, &fname, ctx.child_qname(&fname), lno, lno);
                    c.signature = format!("{name}.{fname}");
                    c.type_ref = Some(name.clone());
                    ctx.add_symbol(c);
                    continue;
                }
                let ftype = parts.get(1).map(|s| s.to_string());
                let attrs: Vec<String> = split_attrs(t);
                let mut f = NewSymbol::new(SymbolKind::Field, &fname, ctx.child_qname(&fname), lno, lno);
                f.signature = collapse_ws(t);
                f.type_ref = ftype;
                f.attrs = attrs;
                f.body_hash = crate::hash::short_hash(t);
                ctx.add_symbol(f);
            }
            ctx.pop_scope();
            i = j + 1;
            continue;
        }
        // datasource url = env("DATABASE_URL")
        if let Some(pos) = line.find("env(\"") {
            let rest = &line[pos + 5..];
            if let Some(end) = rest.find('"') {
                let key = rest[..end].to_string();
                let lno = i as u32 + 1;
                ctx.push_config(&key, "prisma env", lno, None);
            }
        }
        i += 1;
    }
}

/// Splits `@id @default(uuid()) @relation(fields: [a], references: [b])` into attribute strings.
fn split_attrs(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '@' && (i + 1 < bytes.len() && bytes[i + 1] != '@') {
            let start = i;
            let mut depth = 0;
            i += 1;
            while i < bytes.len() {
                match bytes[i] {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    c if c.is_whitespace() && depth == 0 => break,
                    _ => {}
                }
                i += 1;
            }
            out.push(bytes[start..i.min(bytes.len())].iter().collect());
        } else {
            i += 1;
        }
    }
    out
}
