use codegraph_mcp::{call_tool, Engine};
use serde_json::json;

const SOURCE: &str = r#"import Fastify from 'fastify';

export function buildApp() {
    let app = Fastify();
    const other = Fastify();
    const alias = app;
    app.addContentTypeParser('application/too-early', (_r, body, done) => done(null, body));
    app.removeAllContentTypeParsers();
    app.removeContentTypeParser('application/json');
    app.addContentTypeParser(
        'application/json',
        { parseAs: 'buffer', bodyLimit: 262144 },
        (_request, body, done) => done(null, body),
    );
    app.register(async (app) => {
        app.addContentTypeParser('application/sibling', (_r, body, done) => done(null, body));
    });
    other.addContentTypeParser('application/other', (_r, body, done) => done(null, body));
    alias.addContentTypeParser('application/alias', (_r, body, done) => done(null, body));
    if (process.env.NESTED) {
        app.addContentTypeParser('application/nested', (_r, body, done) => done(null, body));
    }
    app.post('/webhook', { bodyLimit: 262144 }, async (request) => request.body);
    app.route({ method: 'PATCH', url: '/patch', handler: async (request) => request.body });
    app.get('/plain', async () => 'ok');
    app.addContentTypeParser('application/after', (_r, body, done) => done(null, body));
    app = Fastify();
    app.post('/rebound', async (request) => request.body);
    return app;
}
"#;

fn trace(engine: &Engine, route: &str) -> String {
    let (text, error) = call_tool(engine, "trace_fullstack_flow", json!({ "api_route_or_call": route, "route_evidence": true }));
    assert!(!error && text.contains("ROUTE ") && text.contains("register :"), "{text}");
    text
}

#[test]
fn route_evidence_shows_recent_preceding_parsers_from_the_same_receiver_and_block() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("main.ts"), SOURCE).unwrap();
    std::fs::write(
        temp.path().join("top.ts"),
        "import Fastify from 'fastify';\nconst app = Fastify();\napp.removeAllContentTypeParsers();\napp.post('/top', async () => 'ok');\n",
    )
    .unwrap();
    let engine = Engine::open(temp.path()).unwrap();
    assert!(trace(&engine, "POST /top").contains("preceding parser :3 | app.removeAllContentTypeParsers()"));

    let text = trace(&engine, "POST /webhook");
    assert_eq!(text.matches("preceding parser").count(), 3, "{text}");
    let remove_all = "preceding parser :8 | app.removeAllContentTypeParsers()";
    let remove_json = "preceding parser :9 | app.removeContentTypeParser('application/json')";
    let add_json = "preceding parser :10-14 | app.addContentTypeParser(";
    let positions = [remove_all, remove_json, add_json].map(|line| text.find(line).unwrap_or_else(|| panic!("missing {line}:\n{text}")));
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    assert!(text.contains("{ parseAs: 'buffer', bodyLimit: 262144 }") && text.contains("done(null, body)"), "{text}");
    for excluded in ["too-early", "sibling", "application/other", "application/alias", "application/nested", "application/after"] {
        assert!(!text.contains(excluded), "{text}");
    }

    assert!(trace(&engine, "PATCH /patch").contains(add_json));
    assert!(!trace(&engine, "GET /plain").contains("preceding parser"));
    assert!(!trace(&engine, "POST /rebound").contains("preceding parser"));
}

#[test]
fn route_evidence_uses_indexed_source_only_for_ambiguous_direct_registrations() {
    let temp = tempfile::tempdir().unwrap();
    let source = r#"import Fastify from 'fastify';
const app = Fastify();
const other = Fastify();
app.register(async (scope) => {
    other.addContentTypeParser('application/other', (_r, body, done) => done(null, body));
    scope.addContentTypeParser('application/json', (_r, body, done) => done(null, body));
    scope.post('/hook', async (r) => r.body); other.post('/prefix/hook', async (r) => r.headers);
}, { prefix: '/prefix' });
app.register(async (scope) => {
    scope.removeContentTypeParser('application/json');
    scope.addContentTypeParser('application/json', { parseAs: 'buffer' }, (_r, body, done) => done(null, body));
    scope.post('/unambiguous', async (r) => r.body);
});
"#;
    std::fs::write(temp.path().join("main.ts"), source).unwrap();
    let engine = Engine::open(temp.path()).unwrap();

    let (ambiguous, error) =
        call_tool(&engine, "trace_fullstack_flow", json!({ "api_route_or_call": "POST /prefix/hook", "route_evidence": true }));
    assert!(!error, "{ambiguous}");
    assert_eq!(ambiguous.matches("ROUTE POST /prefix/hook").count(), 2, "{ambiguous}");
    assert_eq!(ambiguous.matches("indexed source :7").count(), 2, "{ambiguous}");
    assert!(!ambiguous.contains("register :") && !ambiguous.contains("preceding parser"), "{ambiguous}");

    let positive = trace(&engine, "POST /unambiguous");
    assert!(positive.contains("preceding parser :10 | scope.removeContentTypeParser('application/json')"), "{positive}");
    assert!(positive.contains("preceding parser :11 | scope.addContentTypeParser('application/json'"), "{positive}");
}
