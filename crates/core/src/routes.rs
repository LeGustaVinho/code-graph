//! HTTP route normalization shared by backend route declarations and frontend calls.

/// Normalizes a route or URL into a comparable shape:
/// lowercase, no scheme/host/query, parameters collapsed to `{}`, no trailing slash.
///
/// `/api/Orders/{id:int}` → `/api/orders/{}`, `` `${base}/orders/${id}` `` → `{}/orders/{}`
pub fn normalize_route(raw: &str) -> String {
    let mut s = raw.trim().trim_matches('`').trim_matches('"').trim_matches('\'').to_string();
    // Strip scheme + host.
    if let Some(idx) = s.find("://") {
        let rest = &s[idx + 3..];
        s = match rest.find('/') {
            Some(p) => rest[p..].to_string(),
            None => "/".to_string(),
        };
    }
    // Strip query string / fragment.
    if let Some(q) = s.find(['?', '#']) {
        s.truncate(q);
    }
    let mut segments = Vec::new();
    for seg in s.split('/') {
        if seg.is_empty() {
            continue;
        }
        let is_param = seg.starts_with(':')
            || (seg.starts_with('{') && seg.ends_with('}'))
            || (seg.starts_with('<') && seg.ends_with('>'))
            || seg.starts_with("${")
            || seg == "*"
            || (seg.starts_with('[') && seg.ends_with(']') && !seg.contains("controller") && !seg.contains("action"));
        if is_param {
            segments.push("{}".to_string());
        } else if seg.contains("${") || seg.contains('{') {
            // Partially templated segment: `order-${id}`.
            segments.push("{}".to_string());
        } else {
            segments.push(seg.to_ascii_lowercase());
        }
    }
    let leading_param = raw.trim().trim_matches('`').starts_with("${");
    let joined = segments.join("/");
    if leading_param {
        // Already starts with `{}` (base URL variable).
        joined
    } else {
        format!("/{joined}")
    }
}

/// Joins a route prefix and a sub-path (`api/[controller]` + `{id}`).
pub fn join_route(prefix: &str, path: &str) -> String {
    let p = prefix.trim().trim_matches('/');
    let s = path.trim();
    if s.starts_with('/') || s.starts_with('~') {
        // ASP.NET: leading `/` or `~/` on an action route overrides the controller prefix.
        return format!("/{}", s.trim_start_matches('~').trim_matches('/'));
    }
    let s = s.trim_matches('/');
    match (p.is_empty(), s.is_empty()) {
        (true, true) => "/".to_string(),
        (true, false) => format!("/{s}"),
        (false, true) => format!("/{p}"),
        (false, false) => format!("/{p}/{s}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_params_and_hosts() {
        assert_eq!(normalize_route("/api/Orders/{id:int}"), "/api/orders/{}");
        assert_eq!(normalize_route("/api/orders/:id/cancel"), "/api/orders/{}/cancel");
        assert_eq!(normalize_route("`/api/orders/${id}/cancel`"), "/api/orders/{}/cancel");
        assert_eq!(normalize_route("http://localhost:5000/api/orders?x=1"), "/api/orders");
        assert_eq!(normalize_route("`${API_BASE}/profile`"), "{}/profile");
        assert_eq!(normalize_route("/items/<int:item_id>"), "/items/{}");
    }

    #[test]
    fn joins_prefixes() {
        assert_eq!(join_route("api/orders", "{id}"), "/api/orders/{id}");
        assert_eq!(join_route("", "/health"), "/health");
        assert_eq!(join_route("api/orders", "/override"), "/override");
        assert_eq!(join_route("orders", ""), "/orders");
    }
}
