use std::collections::BTreeSet;

use codegraph_graph::{index_project, open, IndexOptions};

const SOURCE: &str = r#"import Fastify, { fastify as create } from 'fastify';
import users from './users';

function handler() { return 'ok'; }
function charge(body: unknown) { return body; }
const other = { register(_callback: any) {}, post(_path: string, _handler: any) {} };
const app = Fastify();

app.register(async (scope, server) => {
    scope.post('/webhook', { bodyLimit: 2048 }, async (req) => charge(req.body));
    server.post('/options-not-instance', handler);
    {
        const scope = other;
        scope.post('/block-shadow', handler);
    }
    [other].forEach(scope => scope.post('/parameter-shadow', handler));
    scope.post('/after-shadow', handler);
    scope.register(async (inner) => {
        inner.get('/x', handler);
        scope.get('/outer', handler);
        app.get('/root-app', handler);
    }, { prefix: '/inner' });
    scope.register(users, { prefix: '/plugins' });
    scope.register(users);
    const registerRoute = (path, h) => scope.get(path, h);
    registerRoute('/wrapped', () => handler());
    {
        const registerRoute = (_path, h) => h();
        registerRoute('/shadowed-helper', () => handler());
    }
}, { prefix: '/one' });

app.register(async (scope) => {
    scope.get('/x', handler);
    registerRoute('/leaked-helper', () => handler());
}, { prefix: '/two' });

create().register(async plugin => {
    plugin.route({ method: 'PUT', url: '/object', handler });
}, { prefix: '/named' });

app.register(async scope => scope.post('/no-prefix', handler));
app.register(async scope => scope.post('/late-prefix', handler), { ...other, prefix: '/last' });
app.register(async scope => scope.post('/unknown-spread', handler), { prefix: '/false', ...other });
app.register(async scope => scope.post('/dynamic-template', handler), { prefix: `/api/${process.env.VERSION}` });

app.register(async scope => {
    scope.post('/dynamic', handler);
    scope.register(async inner => inner.post('/nested-dynamic', handler), { prefix: '/inner' });
    app.get('/root-with-dynamic-child', handler);
}, { prefix: process.env.PREFIX });

other.register(async server => server.post('/other-receiver', handler));
app.register(async scope => {
    scope = other;
    scope.post('/reassigned', handler);
}, { prefix: '/rebound' });

function shadowFactory(Fastify: any) {
    const app = Fastify();
    app.register(async server => server.post('/shadowed-factory', handler));
}

function shadowInstance() {
    const app = Fastify();
    {
        const app = other;
        app.register(async server => server.post('/shadowed-instance', handler));
    }
}
"#;

#[test]
fn inline_fastify_plugins_use_the_lexical_receivers_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("app.ts"), SOURCE).unwrap();
    std::fs::write(root.join("users.ts"), r#"import type { FastifyInstance } from 'fastify';
export default async function users(instance: FastifyInstance) {
    instance.get('/list', () => 'ok');
}"#).unwrap();
    std::fs::write(root.join("mixed.ts"), r#"import express from 'express';
import Fastify from 'fastify';
const registry = { register(_fn: any) {} };
registry.register((server) => {
    const app = express();
    app.get('/express-under-other-register', () => 'ok');
    server.get('/not-fastify', () => 'wrong');
});
const app = Fastify();
app.register((scope) => {
    const router = express.Router();
    router.get('/express-under-fastify', () => 'ok');
    scope.get('/mixed-fastify', () => 'ok');
    {
        const express = () => registry;
        const router = express();
        router.get('/shadowed-express-factory', () => 'wrong');
    }
});"#).unwrap();
    let mut conn = open(root).unwrap();
    index_project(root, &mut conn, IndexOptions::default()).unwrap();

    let mut stmt = conn.prepare("SELECT http_method || ' ' || path || ' ' || framework FROM api_route").unwrap();
    let actual: BTreeSet<String> = stmt.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    let expected = [
        "POST /one/webhook fastify",
        "POST /one/after-shadow fastify",
        "GET /one/inner/x fastify",
        "GET /one/outer fastify",
        "GET /root-app fastify",
        "GET /one/wrapped fastify",
        "GET /one/plugins/list fastify",
        "GET /one/list fastify",
        "GET /two/x fastify",
        "PUT /named/object fastify",
        "POST /no-prefix fastify",
        "POST /last/late-prefix fastify",
        "GET /root-with-dynamic-child fastify",
        "GET /express-under-other-register express",
        "GET /express-under-fastify express",
        "GET /mixed-fastify fastify",
    ]
    .map(str::to_owned)
    .into_iter()
    .collect();
    assert_eq!(actual, expected);

    let webhook_line = SOURCE.lines().position(|line| line.contains("scope.post('/webhook'")).unwrap() as u32 + 1;
    let (file, line, handler, service): (String, u32, String, String) = conn.query_row(
        "SELECT r.file_path, r.line_number, h.name, t.name FROM api_route r JOIN node h ON h.id = r.handler_id \
         JOIN edge e ON e.source_id = h.id JOIN node t ON t.id = e.target_id \
         WHERE r.path = '/one/webhook' AND e.relation = 'CALLS' AND e.confidence = 'exact' AND t.name = 'charge'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ).unwrap();
    assert_eq!((file.as_str(), line, handler.as_str(), service.as_str()), ("app.ts", webhook_line, "POST /one/webhook", "charge"));
}
