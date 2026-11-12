use std::path::Path;

use codegraph_mcp::{call_tool, Engine};
use serde_json::json;

const APP: &str = "apps/example folder/main.ts";
const SOURCE: &str = r#"import Fastify from 'fastify';
import { cancel, shared } from './service.js';

export function buildApp() {
    const app = Fastify();
    async function actor(r: any) {
        if (!r.headers.authorization.startsWith('Bearer ')) throw new Error('unauthorized');
        return r.headers.authorization;
    }
    const route = (method: 'GET' | 'POST', url: string, body: any, handler: any) => {
        return app.route({
            method,
            url,
            schema: { ...(body ? { body } : {}) },
            handler: async (r: any) => {
                const identity = await actor(r);
                if (url === '/another') {
                    unrelated();
                }
                return handler(identity, r);
            },
        });
    };
    const nestedRoute = (method: 'GET' | 'POST', url: string, body: any, handler: any) => {
        return app.route({ method, url, schema: { ...(body ? { response: { body } } : {}) }, handler });
    };
    const override = {};
    const overriddenRoute = (method: 'GET' | 'POST', url: string, body: any, handler: any) => {
        return app.route({ method, url, schema: { body, ...override }, handler });
    };
    const CancelBody = { id: 'uuid', kind: ['solo', 'host'] };
    route('POST', '/api/jobs/cancel', CancelBody, (a: string, r: any) =>
        cancel(a, r.body),
    );
    nestedRoute('POST', '/api/jobs/nested', CancelBody, (a: string, r: any) => cancel(a, r.body));
    overriddenRoute('POST', '/api/jobs/overridden', CancelBody, (a: string, r: any) => cancel(a, r.body));
    app.get('/numeric/42', shared);
    app.post('/v1/numeric/42', shared);
    app.get('/dual', shared);
    app.post('/dual', shared);
    return app;
}

export function cancelFromClient(id: string) {
    return fetch('/api/jobs/cancel', { method: 'POST', body: id });
}

export function postDualFromClient() {
    fetch('/dual', { method: 'POST' });
    return fetch('/dual', { method: 'POST' });
}

export function unlinked() {
    fetch('/ghost', { method: 'GET' });
    fetch('/ghost', { method: 'POST' });
}
"#;

fn trace(engine: &Engine, input: &str, evidence: bool) -> String {
    let (text, error) = call_tool(engine, "trace_fullstack_flow", json!({ "api_route_or_call": input, "route_evidence": evidence }));
    assert!(!error, "{text}");
    text
}

fn fixture(root: &Path) {
    let app = root.join(APP);
    let dir = app.parent().unwrap();
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(&app, SOURCE).unwrap();
    std::fs::write(
        dir.join("service.ts"),
        "export function cancel(a: string, body: unknown) { return { a, body }; }\nexport function shared() { return 'ok'; }",
    )
    .unwrap();
}

#[test]
fn route_evidence_quotes_only_verified_wrapper_body_and_direct_calls() {
    let temp = tempfile::tempdir().unwrap();
    fixture(temp.path());
    let engine = Engine::open(temp.path()).unwrap();
    let line = SOURCE.lines().position(|l| l.contains("route('POST', '/api/jobs/cancel'")).unwrap() + 1;
    let route = trace(&engine, "POST /api/jobs/cancel", true);
    for expected in [
        format!("{APP}:{line}"),
        format!("register :{line}-"),
        "buildApp.route [exact]".into(),
        "| app.route".into(),
        "| ...(body ? { body } : {})".into(),
        "| CancelBody = { id: 'uuid', kind: ['solo', 'host'] }".into(),
        "| const identity = await actor(r);".into(),
        "| return handler(identity, r);".into(),
        "Bearer ".into(),
        "cancel apps/example folder/service.ts:1 [exact]".into(),
    ] {
        assert!(route.contains(&expected), "missing {expected:?}\n{route}");
    }
    assert!(!route.contains("unrelated"), "{route}");
    assert!(trace(&engine, &format!("{APP}:{line}"), true).contains("POST /api/jobs/cancel"));
    assert!(trace(&engine, "POST /api/jobs/cancel", false).contains("backend flow:"));

    let nested = trace(&engine, "POST /api/jobs/nested", true);
    assert!(nested.contains("register :"), "{nested}");
    assert!(!nested.contains("body wiring"), "{nested}");
    assert!(!nested.contains("body input"), "{nested}");
    let overridden = trace(&engine, "POST /api/jobs/overridden", true);
    assert!(overridden.contains("dispatch :") && !overridden.contains("body wiring"), "{overridden}");

    // An edit with unchanged mtime and size evades the normal freshness prefilter.
    let source = temp.path().join(APP);
    let modified = source.metadata().unwrap().modified().unwrap();
    std::fs::write(&source, SOURCE.replace("Bearer ", "Bearor ")).unwrap();
    std::fs::File::options().write(true).open(source).unwrap().set_times(std::fs::FileTimes::new().set_modified(modified)).unwrap();
    let stale = trace(&engine, "POST /api/jobs/cancel", true);
    assert!(stale.contains("registration source unavailable at indexed coordinates") && !stale.contains("Bearor "), "{stale}");
}

#[test]
fn route_matching_preserves_numeric_paths_arity_and_shared_handler_registrations() {
    let temp = tempfile::tempdir().unwrap();
    fixture(temp.path());
    let engine = Engine::open(temp.path()).unwrap();
    let postfix = trace(&engine, "POST /numeric/42", true);
    assert!(postfix.contains("ROUTE POST /v1/numeric/42"), "{postfix}");
    let numeric = trace(&engine, "numeric/42", true);
    assert!(numeric.contains("ROUTE GET /numeric/42"), "{numeric}");
    let arity = trace(&engine, "cancelFromClient/1", true);
    assert!(arity.contains("ROUTE POST /api/jobs/cancel"), "{arity}");
    let dual = trace(&engine, "/dual", true);
    assert!(dual.contains("ROUTE GET /dual") && dual.contains("ROUTE POST /dual"), "{dual}");
    let post_dual = trace(&engine, "postDualFromClient", true);
    assert_eq!(post_dual.matches("ROUTE POST /dual").count(), 1, "{post_dual}");
    assert!(!post_dual.contains("ROUTE GET /dual"), "{post_dual}");
    let unlinked = trace(&engine, "POST /ghost", true);
    assert!(unlinked.contains("UNLINKED POST /ghost") && !unlinked.contains("UNLINKED GET /ghost"), "{unlinked}");
}
