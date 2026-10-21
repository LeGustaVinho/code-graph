//! Vue / Svelte single-file components (`.vue`, `.svelte`).
//!
//! The `<script>` blocks are extracted as TypeScript from a copy of the file in which everything
//! else is blanked (bytes become spaces, newlines stay), so lines and byte offsets are those of
//! the real file. The markup then contributes the component itself (the default export importers
//! get) and what its template uses: child components and the script bindings it calls / reads.

use std::collections::HashSet;

use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SfcKind {
    Vue,
    Svelte,
}

impl SfcKind {
    fn label(self) -> &'static str {
        match self {
            SfcKind::Vue => "vue component",
            SfcKind::Svelte => "svelte component",
        }
    }
}

pub fn sfc_kind(path: &str) -> Option<SfcKind> {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".vue") {
        Some(SfcKind::Vue)
    } else if lower.ends_with(".svelte") {
        Some(SfcKind::Svelte)
    } else {
        None
    }
}

/// Byte ranges of `<script …>` contents and of whole `<style>` / `<script>` / comment blocks.
struct Blocks {
    scripts: Vec<(usize, usize)>,
    non_markup: Vec<(usize, usize)>,
}

fn find_ci(hay: &[u8], from: usize, needle: &str) -> Option<usize> {
    let n = needle.as_bytes();
    if from >= hay.len() || n.len() > hay.len() {
        return None;
    }
    (from..=hay.len() - n.len()).find(|&i| hay[i..i + n.len()].eq_ignore_ascii_case(n))
}

fn blocks(src: &str) -> Blocks {
    let b = src.as_bytes();
    let mut out = Blocks { scripts: Vec::new(), non_markup: Vec::new() };
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"<!--") {
            let end = find_ci(b, i + 4, "-->").map(|e| e + 3).unwrap_or(b.len());
            out.non_markup.push((i, end));
            i = end;
            continue;
        }
        let tag = ["script", "style"].into_iter().find(|t| {
            find_ci(b, i + 1, t) == Some(i + 1) && b.get(i + 1 + t.len()).map(|c| c.is_ascii_whitespace() || *c == b'>').unwrap_or(false)
        });
        let Some(tag) = tag else {
            i += 1;
            continue;
        };
        let Some(open_end) = find_ci(b, i, ">").map(|e| e + 1) else { break };
        let close = find_ci(b, open_end, &format!("</{tag}")).unwrap_or(b.len());
        let end = find_ci(b, close, ">").map(|e| e + 1).unwrap_or(b.len());
        if tag == "script" {
            out.scripts.push((open_end, close));
        }
        out.non_markup.push((i, end));
        i = end;
    }
    out
}

/// Keeps only the given ranges; every other byte (except newlines) becomes a space.
fn keep_only(src: &str, keep: &[(usize, usize)]) -> String {
    let mut out = src.as_bytes().to_vec();
    let mut k = 0;
    for (i, byte) in out.iter_mut().enumerate() {
        while k < keep.len() && keep[k].1 <= i {
            k += 1;
        }
        let kept = k < keep.len() && keep[k].0 <= i;
        if !kept && *byte != b'\n' {
            *byte = b' ';
        }
    }
    // Only whole multi-byte characters were replaced (all their bytes are outside the ranges).
    String::from_utf8(out).unwrap_or_default()
}

/// The script blocks, in place.
pub fn script_source(src: &str) -> String {
    keep_only(src, &blocks(src).scripts)
}

/// The markup (template), in place: scripts, styles and comments blanked.
fn markup_source(src: &str) -> String {
    let bl = blocks(src);
    let mut keep = Vec::new();
    let mut start = 0;
    for (a, b) in bl.non_markup {
        if a > start {
            keep.push((start, a));
        }
        start = b;
    }
    if start < src.len() {
        keep.push((start, src.len()));
    }
    keep_only(src, &keep)
}

/// `user-card` / `user_card` → `UserCard`.
fn pascal(name: &str) -> String {
    name.split(['-', '_', '.'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
        })
        .collect()
}

const VUE_LIFECYCLE: &[&str] = &[
    "beforeCreate",
    "created",
    "beforeMount",
    "mounted",
    "beforeUpdate",
    "updated",
    "beforeUnmount",
    "unmounted",
    "beforeDestroy",
    "destroyed",
    "activated",
    "deactivated",
    "errorCaptured",
    "serverPrefetch",
    "data",
    "setup",
    "render",
];

pub fn is_vue_lifecycle(name: &str) -> bool {
    VUE_LIFECYCLE.contains(&name)
}

const TEMPLATE_KEYWORDS: &[&str] = &[
    "true", "false", "null", "undefined", "in", "of", "as", "typeof", "instanceof", "new", "this", "if", "else", "each", "await", "then",
    "catch", "key", "html", "const", "debug", "let", "return",
];

/// Adds the component symbol (unless the script default-exports one) and the template's uses.
pub(crate) fn extract_markup(ctx: &mut Ctx, original: &str, kind: SfcKind) {
    let markup = markup_source(original);
    let module_id = ctx.module_id();
    let line_count = ctx.facts.line_count;
    // The default export of the script (Vue options object / `defineComponent`), else the file.
    let existing = ctx
        .facts
        .symbols
        .iter()
        .find(|s| s.parent_id.as_deref() == Some(module_id.as_str()) && s.attrs.iter().any(|a| a == "export default"))
        .map(|s| (s.id.clone(), s.qualified_name.clone()));
    let (component_id, component_qname) = match existing {
        Some(c) => c,
        None => {
            let name = pascal(&file_stem(ctx.path));
            let mut s = NewSymbol::new(SymbolKind::Class, &name, ctx.child_qname(&name), 1, line_count);
            s.signature = format!("<{name}> ({})", kind.label());
            s.is_exported = true;
            s.visibility = "public".into();
            s.attrs = vec!["export default".into(), kind.label().into()];
            s.body_hash = crate::hash::content_hash(original.as_bytes());
            let qname = s.qualified_name.clone();
            let id = ctx.add_symbol(s);
            // SvelteKit `src/routes/users/[id]/+page.svelte` is the page at `/users/:id`.
            if let Some(path) = sveltekit_page(ctx.path) {
                ctx.push_route(&id, "PAGE", &path, "sveltekit", 1);
            }
            (id, qname)
        }
    };
    // Bindings the template can see: script top-level declarations, imports and the members of
    // an options-API component (reached through `this`).
    let members: HashSet<String> =
        ctx.facts.symbols.iter().filter(|s| s.parent_id.as_deref() == Some(component_id.as_str())).map(|s| s.name.clone()).collect();
    let mut known: HashSet<String> =
        ctx.facts.symbols.iter().filter(|s| s.parent_id.as_deref() == Some(module_id.as_str())).map(|s| s.name.clone()).collect();
    known.extend(ctx.facts.imports.iter().filter_map(|i| i.local.clone()));
    let component_name = component_qname.rsplit('.').next().unwrap_or(&component_qname).to_string();

    ctx.push_scope(Scope { symbol_id: component_id, kind: Some(SymbolKind::Class), qname: component_qname, ..Default::default() });
    let line_starts: Vec<usize> = std::iter::once(0).chain(markup.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let line_at = |off: usize| line_starts.partition_point(|s| *s <= off) as u32;
    let b = markup.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'<' if b.get(i + 1).map(|c| c.is_ascii_alphabetic()).unwrap_or(false) => {
                let end = tag_end(b, i);
                let tag = &markup[i + 1..end];
                let name_len = tag.find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>').unwrap_or(tag.len());
                let name = &tag[..name_len];
                // `<UserCard>` / `<user-card>` / `<Menu.Item>`: a child component.
                if !name.contains(':') && (name.starts_with(|c: char| c.is_ascii_uppercase()) || name.contains('-')) {
                    let (recv, simple) = match name.rsplit_once('.') {
                        Some((r, s)) => (Some(r.to_string()), s.to_string()),
                        None => (None, pascal(name)),
                    };
                    ctx.push_ref(RefKind::Jsx, &simple, recv, None, None, line_at(i), None);
                }
                for (attr, value, off) in attributes(tag, i + 1) {
                    // `data-testid="save"`: what Playwright's `getByTestId('save')` finds.
                    if crate::extract::typescript::TESTID_ATTRS.contains(&attr.as_str()) && !value.contains(['{', '}']) {
                        let enclosing = ctx.enclosing();
                        ctx.facts.literals.push(Literal { value: value.clone(), literal_type: "testid".into(), line: line_at(off), column: 1, enclosing });
                        continue;
                    }
                    let event = attr.starts_with('@') || attr.starts_with("v-on:") || attr.starts_with("on:");
                    let bound = event
                        || attr.starts_with(':')
                        || attr.starts_with("v-")
                        || attr.starts_with("bind:")
                        || value.trim_start().starts_with('{');
                    if bound && !attr.starts_with("v-slot") && !attr.starts_with('#') && !attr.starts_with("let:") {
                        // `v-for="item in items"`: only the iterated expression is a use.
                        let expr = if attr == "v-for" { value.split_once(" in ").or(value.split_once(" of ")).map(|(_, r)| r).unwrap_or(&value) } else { &value };
                        let expr = expr.trim().trim_start_matches('{').trim_end_matches('}');
                        expression_refs(ctx, expr, line_at(off), event, &known, &members, &component_name);
                    }
                }
                i = end;
            }
            // Vue `{{ expr }}` / Svelte `{expr}` / `{#each items as item}` text interpolations.
            b'{' => {
                let close = matching_brace(b, i);
                let inner = markup[i + 1..close].trim_start_matches('{').trim_end_matches('}');
                let inner = inner.trim_start_matches(['#', ':', '@', '/']);
                let inner = inner.split_once(" as ").map(|(l, _)| l).unwrap_or(inner);
                expression_refs(ctx, inner, line_at(i), false, &known, &members, &component_name);
                i = close + 1;
            }
            _ => i += 1,
        }
    }
    ctx.pop_scope();
}

/// `src/routes/users/[id]/+page.svelte` → `/users/:id` (groups `(app)` are not path segments).
fn sveltekit_page(path: &str) -> Option<String> {
    let p = path.replace('\\', "/");
    let (before, file) = p.rsplit_once('/')?;
    if file != "+page.svelte" {
        return None;
    }
    let (_, dirs) = before.split_once("routes").filter(|(a, _)| a.is_empty() || a.ends_with('/'))?;
    let segs: Vec<String> = dirs
        .split('/')
        .filter(|s| !s.is_empty() && !(s.starts_with('(') && s.ends_with(')')))
        .map(|s| match s.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
            Some(param) => format!(":{}", param.trim_start_matches("...")),
            None => s.to_string(),
        })
        .collect();
    Some(format!("/{}", segs.join("/")))
}

fn tag_end(b: &[u8], start: usize) -> usize {
    let mut quote = 0u8;
    let mut depth = 0i32;
    for (j, c) in b.iter().enumerate().skip(start + 1) {
        match *c {
            q @ (b'"' | b'\'') if quote == 0 => quote = q,
            q if q == quote => quote = 0,
            b'{' if quote == 0 => depth += 1,
            b'}' if quote == 0 => depth -= 1,
            b'>' if quote == 0 && depth <= 0 => return j,
            _ => {}
        }
    }
    b.len()
}

fn matching_brace(b: &[u8], open: usize) -> usize {
    let mut depth = 0;
    let mut quote = 0u8;
    for (j, c) in b.iter().enumerate().skip(open) {
        match *c {
            q @ (b'"' | b'\'' | b'`') if quote == 0 => quote = q,
            q if q == quote => quote = 0,
            b'{' if quote == 0 => depth += 1,
            b'}' if quote == 0 => {
                depth -= 1;
                if depth == 0 {
                    return j;
                }
            }
            _ => {}
        }
    }
    b.len().saturating_sub(1).max(open)
}

/// `name="value"` / `name='value'` / `name={value}` attributes of a start tag (with offsets).
fn attributes(tag: &str, base: usize) -> Vec<(String, String, usize)> {
    let b = tag.as_bytes();
    let mut out = Vec::new();
    let mut i = tag.find(|c: char| c.is_ascii_whitespace()).unwrap_or(tag.len());
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'/' {
            i += 1;
        }
        let name = tag[start..i].to_string();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            let (vs, ve) = match b.get(i) {
                Some(q @ (b'"' | b'\'')) => {
                    let close = tag[i + 1..].find(*q as char).map(|e| i + 1 + e).unwrap_or(b.len());
                    (i + 1, close)
                }
                Some(b'{') => (i, matching_brace(b, i) + 1),
                _ => {
                    let e = tag[i..].find(|c: char| c.is_ascii_whitespace()).map(|e| i + e).unwrap_or(b.len());
                    (i, e)
                }
            };
            out.push((name, tag[vs.min(b.len())..ve.min(b.len())].to_string(), base + vs));
            i = ve + 1;
        } else if name.is_empty() {
            i += 1;
        }
    }
    out
}

/// Identifiers of a template expression that name script bindings: calls (`save()`, or the
/// handler of an event binding), reads otherwise. Options-API members are reached through `this`.
fn expression_refs(
    ctx: &mut Ctx,
    expr: &str,
    line: u32,
    event: bool,
    known: &HashSet<String>,
    members: &HashSet<String>,
    component: &str,
) {
    let b = expr.as_bytes();
    let bare_handler = event && expr.trim().chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '.');
    let mut i = 0;
    let mut quote = 0u8;
    let mut seen: HashSet<String> = HashSet::new();
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
            i += 1;
            continue;
        }
        if matches!(c, b'"' | b'\'' | b'`') {
            quote = c;
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            let name = &expr[start..i];
            let member_access = expr[..start].trim_end().ends_with('.') && !expr[..start].trim_end().ends_with("..");
            if member_access || TEMPLATE_KEYWORDS.contains(&name) || !seen.insert(name.to_string()) {
                continue;
            }
            let called = bare_handler || expr[i..].trim_start().starts_with('(');
            let kind = if called { RefKind::Call } else { RefKind::Read };
            if members.contains(name) {
                ctx.push_ref(kind, name, Some("this".into()), Some(component.to_string()), None, line, Some("template".into()));
            } else if known.contains(name) {
                ctx.push_ref(kind, name, None, None, None, line, Some("template".into()));
            }
            continue;
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blanking_keeps_lines_and_offsets() {
        let src = "<template>\n  <Foo/>\n</template>\n<script setup lang=\"ts\">\nconst a = 1;\n</script>\n<style>.x{}</style>\n";
        let s = script_source(src);
        assert_eq!(s.len(), src.len());
        assert_eq!(s.lines().count(), src.lines().count());
        assert_eq!(s.lines().nth(4), Some("const a = 1;"));
        let m = markup_source(src);
        assert!(m.contains("<Foo/>") && !m.contains("const a") && !m.contains(".x{}"));
    }

    #[test]
    fn sveltekit_routes() {
        assert_eq!(sveltekit_page("src/routes/(app)/users/[id]/+page.svelte").as_deref(), Some("/users/:id"));
        assert_eq!(sveltekit_page("src/routes/+page.svelte").as_deref(), Some("/"));
        assert_eq!(sveltekit_page("src/lib/Card.svelte"), None);
    }

    #[test]
    fn names() {
        assert_eq!(pascal("user-card"), "UserCard");
        assert_eq!(sfc_kind("src/App.vue"), Some(SfcKind::Vue));
        assert_eq!(sfc_kind("src/App.svelte"), Some(SfcKind::Svelte));
    }
}
