//! C# extractor (tree-sitter-c-sharp).

use tree_sitter::Node;

use super::*;
use crate::routes::join_route;

#[derive(Clone, Default)]
struct ClassInfo {
    name: String,
    kind: Option<SymbolKind>,
    route_prefix: Option<String>,
    is_controller: bool,
    bases: Vec<String>,
}

pub(crate) fn extract(ctx: &mut Ctx, root: Node) {
    visit_container(ctx, root);
    // Statements outside any declaration in a file that failed to parse are parser
    // recovery artifacts, not a C# 9 top-level program.
    if root.has_error() {
        let m = &mut ctx.facts.symbols[0];
        if m.entry_kind.as_deref() == Some("main (top-level statements)") {
            m.is_entrypoint = false;
            m.entry_kind = None;
        }
    }
}

/// Methods the Unity engine / editor invokes by name on MonoBehaviour, ScriptableObject,
/// Editor, EditorWindow and StateMachineBehaviour subclasses.
const UNITY_MESSAGES: &[&str] = &[
    "Awake",
    "Start",
    "Update",
    "FixedUpdate",
    "LateUpdate",
    "OnEnable",
    "OnDisable",
    "OnDestroy",
    "OnValidate",
    "Reset",
    "OnGUI",
    "OnApplicationPause",
    "OnApplicationFocus",
    "OnApplicationQuit",
    "OnTriggerEnter",
    "OnTriggerExit",
    "OnTriggerStay",
    "OnTriggerEnter2D",
    "OnTriggerExit2D",
    "OnTriggerStay2D",
    "OnCollisionEnter",
    "OnCollisionExit",
    "OnCollisionStay",
    "OnCollisionEnter2D",
    "OnCollisionExit2D",
    "OnCollisionStay2D",
    "OnControllerColliderHit",
    "OnMouseDown",
    "OnMouseUp",
    "OnMouseUpAsButton",
    "OnMouseEnter",
    "OnMouseExit",
    "OnMouseOver",
    "OnMouseDrag",
    "OnBecameVisible",
    "OnBecameInvisible",
    "OnDrawGizmos",
    "OnDrawGizmosSelected",
    "OnRenderObject",
    "OnPreRender",
    "OnPostRender",
    "OnPreCull",
    "OnWillRenderObject",
    "OnRenderImage",
    "OnAnimatorMove",
    "OnAnimatorIK",
    "OnParticleCollision",
    "OnParticleTrigger",
    "OnParticleSystemStopped",
    "OnTransformParentChanged",
    "OnTransformChildrenChanged",
    "OnRectTransformDimensionsChange",
    "OnCanvasGroupChanged",
    "OnDidApplyAnimationProperties",
    "OnBeforeTransformParentChanged",
    "OnInspectorGUI",
    "OnSceneGUI",
    "OnSelectionChange",
    "OnHierarchyChange",
    "OnProjectChange",
    "OnFocus",
    "OnLostFocus",
    "OnInspectorUpdate",
    "CreateGUI",
    "CreateInspectorGUI",
    "OnPreviewGUI",
    "HasPreviewGUI",
    "OnBeforeSerialize",
    "OnAfterDeserialize",
    "OnStateEnter",
    "OnStateExit",
    "OnStateUpdate",
    "OnStateMove",
    "OnStateIK",
    "OnStateMachineEnter",
    "OnStateMachineExit",
    "OnWizardCreate",
    "OnWizardUpdate",
    "OnWizardOtherButton",
    "OnPostprocessAllAssets",
    "OnPreprocessTexture",
    "OnPostprocessTexture",
    "OnPreprocessModel",
    "OnPostprocessModel",
    "OnPreprocessAsset",
    "OnPreprocessBuild",
    "OnPostprocessBuild",
    "OnProcessScene",
    "OnServerInitialized",
    "OnConnectedToServer",
];

/// Attributes that make Unity call a method without any code reference.
const UNITY_HOOK_ATTRS: &[&str] = &[
    "MenuItem",
    "InitializeOnLoadMethod",
    "RuntimeInitializeOnLoadMethod",
    "ContextMenu",
    "ContextMenuItem",
    "DidReloadScripts",
    "PostProcessBuild",
    "PostProcessScene",
    "OnOpenAsset",
    "OnOpenAssetAttribute",
    "Shortcut",
    "ClutchShortcut",
    "SettingsProvider",
    "CustomEditor",
    "DrawGizmo",
];

fn uses_unity(ctx: &Ctx) -> bool {
    ctx.facts.imports.iter().any(|i| i.module.starts_with("UnityEngine") || i.module.starts_with("UnityEditor"))
}

fn named_children(node: Node) -> Vec<Node> {
    let mut c = node.walk();
    node.named_children(&mut c).collect()
}

fn visit_container(ctx: &mut Ctx, node: Node) {
    for ch in named_children(node) {
        match ch.kind() {
            "using_directive" => using(ctx, ch),
            "namespace_declaration" | "file_scoped_namespace_declaration" => {
                let short = ch.child_by_field_name("name").map(|n| ctx.text(n).to_string()).unwrap_or_default();
                // `namespace A { namespace B { … } }` declares `A.B`.
                let parent = ctx.scopes[0].qname.clone();
                let name = if parent.is_empty() { short } else { format!("{parent}.{short}") };
                if ctx.facts.namespace.is_none() {
                    ctx.facts.namespace = Some(name.clone());
                }
                let saved_ns = ctx.facts.namespace.replace(name.clone());
                let saved_q = std::mem::replace(&mut ctx.scopes[0].qname, name);
                match ch.child_by_field_name("body") {
                    Some(body) => visit_container(ctx, body),
                    None => visit_container(ctx, ch),
                }
                if ch.kind() == "namespace_declaration" {
                    ctx.scopes[0].qname = saved_q;
                    ctx.facts.namespace = saved_ns;
                }
            }
            "class_declaration" | "struct_declaration" | "interface_declaration" | "record_declaration" | "enum_declaration" => {
                visit_type(ctx, ch, None)
            }
            "global_statement" => {
                let m = &mut ctx.facts.symbols[0];
                if !m.is_entrypoint {
                    m.is_entrypoint = true;
                    m.entry_kind = Some("main (top-level statements)".into());
                }
                walk(ctx, ch);
            }
            "declaration_list" => visit_container(ctx, ch),
            _ => {}
        }
    }
}

fn using(ctx: &mut Ctx, node: Node) {
    let text = ctx.text(node);
    let is_static = text.contains("static ");
    let alias = node.child_by_field_name("name");
    let mut target = None;
    for ch in named_children(node) {
        if Some(ch) == alias {
            continue;
        }
        if matches!(ch.kind(), "qualified_name" | "identifier" | "generic_name" | "alias_qualified_name") {
            target = Some(ctx.text(ch).to_string());
        }
    }
    if let Some(module) = target {
        ctx.facts.imports.push(Import {
            module,
            imported: None,
            local: alias.map(|a| ctx.text(a).to_string()),
            kind: if is_static { ImportKind::UsingStatic } else { ImportKind::Using },
            line: line_of(node),
        });
    }
}

/// Returns (modifiers, attributes) declared directly on a member.
fn mods_attrs(ctx: &Ctx, node: Node) -> (Vec<String>, Vec<String>) {
    let mut mods = Vec::new();
    let mut attrs = Vec::new();
    for ch in named_children(node) {
        match ch.kind() {
            "modifier" => mods.push(ctx.text(ch).trim().to_string()),
            "attribute_list" => {
                for a in named_children(ch) {
                    if a.kind() == "attribute" {
                        attrs.push(collapse_ws(ctx.text(a)));
                    }
                }
            }
            _ => {}
        }
    }
    (mods, attrs)
}

fn record_attr_literals(ctx: &mut Ctx, node: Node) {
    for ch in named_children(node) {
        if ch.kind() == "attribute_list" {
            collect_strings(ctx, ch);
        }
    }
}

fn collect_strings(ctx: &mut Ctx, node: Node) {
    if let Some(s) = string_like(node, ctx.src) {
        let t = if node.kind().starts_with("interpolated") { "template" } else { "string" };
        ctx.push_literal(&s, t, node);
        return;
    }
    for ch in named_children(node) {
        collect_strings(ctx, ch);
    }
}

fn visibility(mods: &[String], default: &str) -> String {
    for m in mods {
        if matches!(m.as_str(), "public" | "private" | "protected" | "internal") {
            return m.clone();
        }
    }
    default.to_string()
}

fn attr_name(attr: &str) -> &str {
    let n = attr.split('(').next().unwrap_or(attr).trim();
    let n = n.rsplit('.').next().unwrap_or(n);
    n.trim_end_matches("Attribute")
}

/// First string literal argument of an attribute text: `HttpGet("{id}")` → `{id}`.
fn attr_string_arg(attr: &str) -> Option<String> {
    let start = attr.find('"')?;
    let rest = &attr[start + 1..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn visit_type(ctx: &mut Ctx, node: Node, outer: Option<&ClassInfo>) {
    let Some(name_node) = node.child_by_field_name("name") else { return };
    let name = ctx.text(name_node).to_string();
    // Grammar 0.23 parses `record struct` as a `record_declaration` with a `struct` token.
    let record_struct = node.kind() == "record_declaration" && {
        let mut c = node.walk();
        let r = node.children(&mut c).any(|ch| !ch.is_named() && ch.kind() == "struct");
        r
    };
    let kind = match node.kind() {
        "interface_declaration" => SymbolKind::Interface,
        "struct_declaration" => SymbolKind::Struct,
        "record_declaration" if record_struct => SymbolKind::Struct,
        "enum_declaration" => SymbolKind::Enum,
        _ => SymbolKind::Class,
    };
    let (mods, attrs) = mods_attrs(ctx, node);
    let mut bases = Vec::new();
    let mut primary_params: Vec<(String, Option<String>)> = Vec::new();
    for ch in named_children(node) {
        match ch.kind() {
            "base_list" => {
                for b in named_children(ch) {
                    let t = if b.kind() == "primary_constructor_base_type" {
                        b.named_child(0).map(|x| ctx.text(x).to_string()).unwrap_or_default()
                    } else if b.kind() == "argument_list" {
                        continue;
                    } else {
                        ctx.text(b).to_string()
                    };
                    if !t.is_empty() {
                        bases.push(collapse_ws(&t));
                    }
                }
            }
            "parameter_list" => {
                for p in named_children(ch) {
                    if p.kind() == "parameter" {
                        if let Some(n) = p.child_by_field_name("name") {
                            let ty = p.child_by_field_name("type").map(|t| ctx.text(t).to_string());
                            primary_params.push((ctx.text(n).to_string(), ty));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let keyword = match node.kind() {
        "interface_declaration" => "interface",
        "struct_declaration" => "struct",
        "record_declaration" if record_struct => "record struct",
        "record_declaration" => "record",
        "enum_declaration" => "enum",
        _ => "class",
    };
    let tparams = node.child_by_field_name("type_parameters").map(|t| ctx.text(t).to_string()).unwrap_or_default();
    let mut signature = format!("{keyword} {name}{tparams}");
    if !primary_params.is_empty() {
        let ps: Vec<String> =
            primary_params.iter().map(|(n, t)| format!("{} {}", t.clone().unwrap_or_default(), n).trim().to_string()).collect();
        signature.push_str(&format!("({})", ps.join(", ")));
    }
    if !bases.is_empty() {
        signature.push_str(&format!(" : {}", bases.join(", ")));
    }
    let qname = ctx.child_qname(&name);
    let default_vis = if outer.is_some() { "private" } else { "internal" };
    let mut sym = NewSymbol::new(kind, &name, qname.clone(), line_of(node), end_line_of(node));
    sym.signature = signature;
    sym.visibility = visibility(&mods, default_vis);
    sym.is_static = mods.iter().any(|m| m == "static");
    sym.is_abstract = mods.iter().any(|m| m == "abstract") || kind == SymbolKind::Interface;
    sym.is_exported = sym.visibility == "public" || sym.visibility == "internal";
    sym.is_test = attrs.iter().any(|a| matches!(attr_name(a), "TestClass" | "TestFixture"));
    sym.docstring = doc_comment(node, ctx.src);
    sym.bases = bases.clone();
    sym.body_hash = token_hash(node, ctx.src);
    let route_prefix = attrs.iter().find(|a| attr_name(a) == "Route").and_then(|a| attr_string_arg(a));
    let is_controller = name.ends_with("Controller")
        || attrs.iter().any(|a| matches!(attr_name(a), "ApiController" | "Controller"))
        || bases.iter().any(|b| b.starts_with("Controller") || b.starts_with("ControllerBase"));
    sym.attrs = attrs;
    let id = ctx.add_symbol(sym);

    // Collect field / property types for receiver inference.
    let mut fields: HashMap<String, Option<String>> = HashMap::new();
    for (n, t) in &primary_params {
        fields.insert(n.clone(), t.clone());
    }
    let body = node.child_by_field_name("body");
    if let Some(body) = body {
        for m in named_children(body) {
            match m.kind() {
                "field_declaration" | "event_field_declaration" => {
                    if let Some(vd) = named_children(m).into_iter().find(|c| c.kind() == "variable_declaration") {
                        let ty = vd.child_by_field_name("type").map(|t| ctx.text(t).to_string());
                        for d in named_children(vd) {
                            if d.kind() == "variable_declarator" {
                                if let Some(n) = declarator_name(d) {
                                    fields.insert(ctx.text(n).to_string(), ty.clone());
                                }
                            }
                        }
                    }
                }
                "property_declaration" => {
                    if let Some(n) = m.child_by_field_name("name") {
                        let ty = m.child_by_field_name("type").map(|t| ctx.text(t).to_string());
                        fields.insert(ctx.text(n).to_string(), ty);
                    }
                }
                _ => {}
            }
        }
    }

    ctx.push_scope(Scope { symbol_id: id.clone(), kind: Some(kind), qname: qname.clone(), fields, ..Default::default() });
    record_attr_literals(ctx, node);
    let info = ClassInfo { name: name.clone(), kind: Some(kind), route_prefix, is_controller, bases };
    if let Some(body) = body {
        if kind == SymbolKind::Enum {
            for m in named_children(body) {
                if m.kind() == "enum_member_declaration" {
                    if let Some(n) = m.child_by_field_name("name") {
                        let mn = ctx.text(n).to_string();
                        let mut s = NewSymbol::new(SymbolKind::Const, &mn, ctx.child_qname(&mn), line_of(m), end_line_of(m));
                        s.signature = format!("{name}.{mn}");
                        s.value_text = Some(collapse_ws(ctx.text(m)));
                        s.type_ref = Some(name.clone());
                        s.is_static = true;
                        s.body_hash = token_hash(m, ctx.src);
                        ctx.add_symbol(s);
                    }
                }
            }
        } else {
            for m in named_children(body) {
                match m.kind() {
                    "method_declaration" => visit_callable(ctx, m, SymbolKind::Method, &info),
                    "constructor_declaration" => visit_callable(ctx, m, SymbolKind::Constructor, &info),
                    "property_declaration" => visit_property(ctx, m, &info),
                    "field_declaration" | "event_field_declaration" => visit_field(ctx, m, &info),
                    "class_declaration" | "struct_declaration" | "interface_declaration" | "record_declaration" | "enum_declaration" => {
                        visit_type(ctx, m, Some(&info))
                    }
                    "operator_declaration" | "conversion_operator_declaration" | "indexer_declaration" | "destructor_declaration" => {
                        // Not modelled as symbols, but their bodies still produce references.
                        walk(ctx, m);
                    }
                    _ => {}
                }
            }
        }
    }
    ctx.pop_scope();
}

fn declarator_name(d: Node) -> Option<Node> {
    d.child_by_field_name("name").or_else(|| {
        let mut c = d.walk();
        let r = d.named_children(&mut c).find(|n| n.kind() == "identifier");
        r
    })
}

fn declarator_value(d: Node) -> Option<Node> {
    let name = declarator_name(d);
    let mut c = d.walk();
    let children: Vec<Node> = d.named_children(&mut c).collect();
    let last = children.into_iter().rev().find(|n| Some(*n) != name && n.kind() != "bracketed_argument_list")?;
    if last.kind() == "equals_value_clause" {
        last.named_child(0)
    } else {
        Some(last)
    }
}

fn visit_field(ctx: &mut Ctx, node: Node, cls: &ClassInfo) {
    let (mods, attrs) = mods_attrs(ctx, node);
    let Some(vd) = named_children(node).into_iter().find(|c| c.kind() == "variable_declaration") else { return };
    let ty = vd.child_by_field_name("type").map(|t| ctx.text(t).to_string());
    let is_const = mods.iter().any(|m| m == "const");
    let default_vis = if cls.kind == Some(SymbolKind::Interface) { "public" } else { "private" };
    for d in named_children(vd) {
        if d.kind() != "variable_declarator" {
            continue;
        }
        let Some(n) = declarator_name(d) else { continue };
        let name = ctx.text(n).to_string();
        let value = declarator_value(d);
        let mut s = NewSymbol::new(
            if is_const { SymbolKind::Const } else { SymbolKind::Field },
            &name,
            ctx.child_qname(&name),
            line_of(node),
            end_line_of(node),
        );
        let tys = ty.clone().unwrap_or_default();
        s.signature = match value {
            Some(v) if is_const => format!("const {tys} {name} = {}", ctx.text(v)),
            _ => format!("{tys} {name}"),
        };
        s.type_ref = ty.clone();
        s.value_text = value.map(|v| ctx.text(v).to_string());
        s.visibility = visibility(&mods, default_vis);
        s.is_static = is_const || mods.iter().any(|m| m == "static");
        s.attrs = attrs.clone();
        s.body_hash = token_hash(d, ctx.src);
        let id = ctx.add_symbol(s);
        if let Some(v) = value {
            ctx.push_scope(Scope { symbol_id: id, qname: ctx.child_qname(&name), ..Default::default() });
            walk(ctx, v);
            ctx.pop_scope();
        }
    }
}

fn visit_property(ctx: &mut Ctx, node: Node, cls: &ClassInfo) {
    let Some(n) = node.child_by_field_name("name") else { return };
    let name = ctx.text(n).to_string();
    let (mods, attrs) = mods_attrs(ctx, node);
    let ty = node.child_by_field_name("type").map(|t| ctx.text(t).to_string());
    let default_vis = if cls.kind == Some(SymbolKind::Interface) { "public" } else { "private" };
    let accessors = node.child_by_field_name("accessors");
    let acc_text = accessors
        .map(|a| {
            let mut parts = Vec::new();
            for acc in named_children(a) {
                if acc.kind() == "accessor_declaration" {
                    let t = acc.child_by_field_name("name").map(|x| ctx.text(x)).unwrap_or("");
                    let (am, _) = mods_attrs(ctx, acc);
                    let prefix = if am.is_empty() { String::new() } else { format!("{} ", am.join(" ")) };
                    parts.push(format!("{prefix}{t};"));
                }
            }
            format!(" {{ {} }}", parts.join(" "))
        })
        .unwrap_or_else(|| " { get; }".into());
    let mut s = NewSymbol::new(SymbolKind::Property, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
    s.signature = format!("{} {}{}", ty.clone().unwrap_or_default(), name, acc_text);
    s.type_ref = ty;
    s.visibility = visibility(&mods, default_vis);
    s.is_static = mods.iter().any(|m| m == "static");
    s.is_abstract = mods.iter().any(|m| m == "abstract");
    s.attrs = attrs;
    s.docstring = doc_comment(node, ctx.src);
    s.body_hash = token_hash(node, ctx.src);
    s.complexity = cyclomatic(node, ctx.src, ctx.lang);
    let value = node.child_by_field_name("value");
    s.value_text = value.map(|v| ctx.text(v).to_string());
    let id = ctx.add_symbol(s);
    ctx.push_scope(Scope {
        symbol_id: id,
        kind: Some(SymbolKind::Property),
        qname: ctx.child_qname(&name),
        is_function: true,
        ..Default::default()
    });
    record_attr_literals(ctx, node);
    for ch in named_children(node) {
        match ch.kind() {
            "accessor_list" => {
                for acc in named_children(ch) {
                    for body in named_children(acc) {
                        if matches!(body.kind(), "block" | "arrow_expression_clause") {
                            walk(ctx, body);
                        }
                    }
                }
            }
            // `string Name => …;` exposes its body as the `value` field, walked below.
            "arrow_expression_clause" if Some(ch) != value => walk(ctx, ch),
            _ => {}
        }
    }
    if let Some(v) = value {
        walk(ctx, v);
    }
    ctx.pop_scope();
}

fn parse_params(ctx: &Ctx, node: Option<Node>) -> (ParamInfo, String, Vec<(String, Option<String>)>) {
    let Some(node) = node else { return (ParamInfo::default(), String::new(), Vec::new()) };
    let mut list = Vec::new();
    let mut locals = Vec::new();
    // `params T[] xs` is not wrapped in a `parameter` node: its `params` token, `type` and
    // `name` sit directly in the parameter list.
    let mut in_params = false;
    let mut params_ty: Option<String> = None;
    let mut c = node.walk();
    let kids: Vec<(Option<&str>, Node)> =
        node.children(&mut c).enumerate().map(|(i, ch)| (node.field_name_for_child(i as u32), ch)).collect();
    for (field, p) in kids {
        if !p.is_named() && p.kind() == "params" {
            in_params = true;
            params_ty = None;
            continue;
        }
        if in_params && p.kind() != "parameter" {
            match field {
                Some("type") => params_ty = Some(ctx.text(p).to_string()),
                Some("name") => {
                    let name = ctx.text(p).to_string();
                    locals.push((name.clone(), params_ty.clone()));
                    list.push(Param { name, ty: params_ty.take(), optional: false, variadic: true });
                    in_params = false;
                }
                _ => {}
            }
            continue;
        }
        if p.kind() != "parameter" {
            continue;
        }
        let name = p.child_by_field_name("name").map(|n| ctx.text(n).to_string()).unwrap_or_default();
        let ty = p.child_by_field_name("type").map(|t| ctx.text(t).to_string());
        let text = ctx.text(p);
        let mut is_this = false;
        let mut variadic = false;
        let mut optional = false;
        let mut pc = p.walk();
        for ch in p.children(&mut pc) {
            match ch.kind() {
                "modifier" | "parameter_modifier" => match ctx.text(ch).trim() {
                    "this" => is_this = true,
                    "params" => variadic = true,
                    _ => {}
                },
                "this" => is_this = true,
                "params" => variadic = true,
                // A default value (not an `=` inside an attribute argument).
                "equals_value_clause" => optional = true,
                _ => {}
            }
        }
        if text.trim_start().starts_with("this ") {
            is_this = true;
        }
        locals.push((name.clone(), ty.clone()));
        if is_this {
            continue;
        }
        list.push(Param { name, ty, optional, variadic });
    }
    let inner = strip_parens(ctx.text(node));
    (ParamInfo::from_list(list), collapse_ws(inner), locals)
}

fn visit_callable(ctx: &mut Ctx, node: Node, kind: SymbolKind, cls: &ClassInfo) {
    let name = node.child_by_field_name("name").map(|n| ctx.text(n).to_string()).unwrap_or_else(|| cls.name.clone());
    let (mods, attrs) = mods_attrs(ctx, node);
    let (params, ptext, locals) = parse_params(ctx, node.child_by_field_name("parameters"));
    let ret = node.child_by_field_name("returns").or_else(|| node.child_by_field_name("type")).map(|t| ctx.text(t).to_string());
    let tparams = node.child_by_field_name("type_parameters").map(|t| ctx.text(t).to_string()).unwrap_or_default();
    let body = node.child_by_field_name("body");
    let arrow = named_children(node).into_iter().find(|c| c.kind() == "arrow_expression_clause");
    let mut s = NewSymbol::new(kind, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
    s.signature = match &ret {
        Some(r) => format!("{name}{tparams}({ptext}): {r}"),
        None => format!("{name}{tparams}({ptext})"),
    };
    let is_interface = cls.kind == Some(SymbolKind::Interface);
    s.visibility = visibility(&mods, if is_interface { "public" } else { "private" });
    s.is_static = mods.iter().any(|m| m == "static");
    s.is_async = mods.iter().any(|m| m == "async");
    s.is_abstract = mods.iter().any(|m| m == "abstract") || (body.is_none() && arrow.is_none());
    s.is_exported = s.visibility == "public";
    s.type_ref = ret.clone();
    s.params = Some(params);
    s.docstring = doc_comment(node, ctx.src);
    s.complexity = cyclomatic(node, ctx.src, ctx.lang);
    s.body_hash = token_hash(node, ctx.src);
    s.is_test = attrs.iter().any(|a| matches!(attr_name(a), "Fact" | "Theory" | "Test" | "TestMethod" | "TestCase" | "DataTestMethod"));

    // Routing / entrypoints.
    let mut http_method: Option<String> = None;
    let mut method_path: Option<String> = None;
    for a in &attrs {
        let an = attr_name(a);
        if let Some(m) = an.strip_prefix("Http") {
            if matches!(m, "Get" | "Post" | "Put" | "Delete" | "Patch" | "Head" | "Options") {
                http_method = Some(m.to_ascii_uppercase());
                if let Some(p) = attr_string_arg(a) {
                    method_path = Some(p);
                }
            }
        } else if an == "Route" {
            method_path = attr_string_arg(a).or(method_path);
        }
    }
    let mut route: Option<(String, String)> = None;
    if cls.is_controller && (http_method.is_some() || method_path.is_some()) && kind == SymbolKind::Method {
        let ctrl = cls.name.trim_end_matches("Controller");
        let prefix = cls.route_prefix.clone().unwrap_or_default().replace("[controller]", ctrl).replace("[action]", &name);
        let sub = method_path.clone().unwrap_or_default().replace("[action]", &name);
        let path = join_route(&prefix, &sub);
        let method = http_method.clone().unwrap_or_else(|| "ANY".into());
        s.is_entrypoint = true;
        s.entry_kind = Some(format!("route {method} {path}"));
        route = Some((method, path));
    }
    if name == "Main" && s.is_static {
        s.is_entrypoint = true;
        s.entry_kind = Some("main".into());
    }
    if attrs.iter().any(|a| matches!(attr_name(a), "Function" | "FunctionName")) {
        s.is_entrypoint = true;
        s.entry_kind = Some("serverless function".into());
    }
    if !s.is_entrypoint && uses_unity(ctx) {
        if let Some(a) = attrs.iter().find(|a| UNITY_HOOK_ATTRS.contains(&attr_name(a))) {
            s.is_entrypoint = true;
            s.entry_kind = Some(format!("unity hook [{}]", attr_name(a)));
        } else if kind == SymbolKind::Method && !cls.bases.is_empty() && UNITY_MESSAGES.contains(&name.as_str()) {
            s.is_entrypoint = true;
            s.entry_kind = Some(format!("unity message {name}"));
        }
    }
    let is_exception_filter = cls.bases.iter().any(|b| {
        let b = clean_type(b);
        matches!(b.as_str(), "IExceptionFilter" | "IAsyncExceptionFilter" | "ExceptionFilterAttribute" | "IExceptionHandler")
    }) && matches!(name.as_str(), "OnException" | "OnExceptionAsync" | "TryHandleAsync");
    s.attrs = attrs;
    let line = line_of(node);
    let id = ctx.add_symbol(s);
    if let Some((m, p)) = route {
        ctx.push_route(&id, &m, &p, "aspnet", line);
    }
    if is_exception_filter {
        ctx.facts.global_handlers.push(GlobalHandler { handler: id.clone(), exception: "*".into(), framework: "aspnet".into(), line });
    }

    let mut scope = Scope { symbol_id: id, kind: Some(kind), qname: ctx.child_qname(&name), is_function: true, ..Default::default() };
    for (n, t) in locals {
        scope.locals.insert(n, t);
    }
    ctx.push_scope(scope);
    record_attr_literals(ctx, node);
    // Parameter attributes like [FromBody] do not matter; default values may hold literals.
    if let Some(p) = node.child_by_field_name("parameters") {
        for param in named_children(p) {
            for ch in named_children(param) {
                if ch.kind() == "equals_value_clause" {
                    walk(ctx, ch);
                }
            }
        }
    }
    for ch in named_children(node) {
        if ch.kind() == "constructor_initializer" {
            walk(ctx, ch);
        }
    }
    if let Some(b) = body {
        walk(ctx, b);
    }
    if let Some(a) = arrow {
        walk(ctx, a);
    }
    ctx.pop_scope();
}

// ---------------------------------------------------------------------------
// Statement / expression walker
// ---------------------------------------------------------------------------

fn walk(ctx: &mut Ctx, node: Node) {
    match node.kind() {
        "variable_declaration" => local_decl(ctx, node),
        "local_function_statement" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = ctx.text(n).to_string();
                ctx.declare_local(&name, None);
            }
            if let Some(p) = node.child_by_field_name("parameters") {
                let (_, _, locals) = parse_params(ctx, Some(p));
                for (n, t) in locals {
                    ctx.declare_local(&n, t);
                }
            }
            for ch in named_children(node) {
                if matches!(ch.kind(), "block" | "arrow_expression_clause") {
                    walk(ctx, ch);
                }
            }
        }
        "invocation_expression" => call(ctx, node),
        "object_creation_expression" => new_expr(ctx, node),
        "throw_statement" | "throw_expression" => throw(ctx, node),
        "try_statement" => try_stmt(ctx, node),
        "assignment_expression" => {
            if let Some(left) = node.child_by_field_name("left") {
                write_target(ctx, left);
                let op = node.child_by_field_name("operator").map(|o| ctx.text(o)).unwrap_or("=");
                if op != "=" {
                    walk(ctx, left);
                }
            }
            if let Some(right) = node.child_by_field_name("right") {
                walk(ctx, right);
            }
        }
        "postfix_unary_expression" | "prefix_unary_expression" => {
            let t = ctx.text(node);
            if t.contains("++") || t.contains("--") {
                if let Some(op) = node.named_child(0) {
                    write_target(ctx, op);
                }
            }
            walk_children(ctx, node);
        }
        "element_access_expression" => {
            element_access(ctx, node);
            walk_children(ctx, node);
        }
        "member_access_expression" => {
            let expr = node.child_by_field_name("expression");
            let name = node.child_by_field_name("name").map(|n| ctx.text(n).to_string());
            match (expr, name) {
                (Some(e), Some(n)) if ctx.text(e) == "this" => {
                    let cls = ctx.class_name();
                    ctx.push_ref(RefKind::Read, &n, Some("this".into()), cls, None, line_of(node), None);
                }
                (Some(e), _) => walk(ctx, e),
                _ => {}
            }
        }
        "identifier" => read_ident(ctx, node),
        "await_expression" => {
            ctx.mark_await();
            walk_children(ctx, node);
        }
        "lambda_expression" | "anonymous_method_expression" => {
            if let Some(p) = node.child_by_field_name("parameters") {
                if p.kind() == "parameter_list" {
                    let (_, _, locals) = parse_params(ctx, Some(p));
                    for (n, t) in locals {
                        ctx.declare_local(&n, t);
                    }
                } else {
                    let n = ctx.text(p).to_string();
                    ctx.declare_local(&n, None);
                }
            }
            for ch in named_children(node) {
                if ch.kind() == "implicit_parameter" || ch.kind() == "identifier" && Some(ch) != node.child_by_field_name("body") {
                    let n = ctx.text(ch).to_string();
                    ctx.declare_local(&n, None);
                }
            }
            if let Some(b) = node.child_by_field_name("body") {
                walk(ctx, b);
            }
        }
        "foreach_statement" => {
            if let Some(l) = node.child_by_field_name("left") {
                let ty = node.child_by_field_name("type").map(|t| ctx.text(t).to_string()).filter(|t| t != "var");
                if l.kind() == "identifier" {
                    let n = ctx.text(l).to_string();
                    ctx.declare_local(&n, ty);
                }
            }
            if let Some(r) = node.child_by_field_name("right") {
                walk(ctx, r);
            }
            if let Some(b) = node.child_by_field_name("body") {
                walk(ctx, b);
            }
        }
        "declaration_expression" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = ctx.text(n).to_string();
                let ty = node.child_by_field_name("type").map(|t| ctx.text(t).to_string()).filter(|t| t != "var");
                ctx.declare_local(&name, ty);
            }
        }
        "declaration_pattern" | "var_pattern" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = ctx.text(n).to_string();
                ctx.declare_local(&name, node.child_by_field_name("type").map(|t| ctx.text(t).to_string()));
            }
        }
        "string_literal" | "verbatim_string_literal" | "raw_string_literal" => {
            let v = string_value(node, ctx.src);
            ctx.push_literal(&v, "string", node);
        }
        "interpolated_string_expression" => {
            let v = template_text(node, ctx.src);
            ctx.push_literal(&v, "template", node);
            walk_children(ctx, node);
        }
        "integer_literal" | "real_literal" => {
            let v = ctx.text(node).to_string();
            ctx.push_literal(&v, "number", node);
        }
        "attribute_list"
        | "type_argument_list"
        | "predefined_type"
        | "qualified_name"
        | "generic_name"
        | "typeof_expression"
        | "parameter_list"
        | "comment"
        | "class_declaration"
        | "struct_declaration"
        | "interface_declaration"
        | "record_declaration"
        | "enum_declaration"
        | "using_directive" => {}
        _ => walk_children(ctx, node),
    }
}

fn walk_children(ctx: &mut Ctx, node: Node) {
    let mut c = node.walk();
    let children: Vec<(Option<String>, Node)> = node
        .children(&mut c)
        .enumerate()
        .filter_map(|(i, ch)| if ch.is_named() { Some((node.field_name_for_child(i as u32).map(|s| s.to_string()), ch)) } else { None })
        .collect();
    for (field, ch) in children {
        if matches!(field.as_deref(), Some("type") | Some("returns") | Some("type_parameters") | Some("type_arguments")) {
            continue;
        }
        walk(ctx, ch);
    }
}

fn read_ident(ctx: &mut Ctx, node: Node) {
    let name = ctx.text(node).to_string();
    if ctx.is_local(&name) {
        return;
    }
    if let Some(p) = node.parent() {
        match p.kind() {
            "variable_declarator"
            | "parameter"
            | "name_colon"
            | "name_equals"
            | "catch_declaration"
            | "base_list"
            | "attribute"
            | "type_parameter"
            | "label_statement"
            | "goto_statement" => return,
            _ => {}
        }
    }
    if ctx.is_field(&name) {
        let cls = ctx.class_name();
        ctx.push_ref(RefKind::Read, &name, Some("this".into()), cls, None, line_of(node), None);
    } else {
        ctx.push_ref(RefKind::Read, &name, None, None, None, line_of(node), None);
    }
}

fn infer_expr_type(ctx: &Ctx, value: Node) -> Option<String> {
    match value.kind() {
        "object_creation_expression" => value.child_by_field_name("type").map(|t| ctx.text(t).to_string()),
        "cast_expression" => value.child_by_field_name("type").map(|t| ctx.text(t).to_string()),
        "await_expression" => value.named_child(0).and_then(|v| infer_expr_type(ctx, v)),
        "invocation_expression" => {
            // GetRequiredService<T>() / GetService<T>()
            let f = value.child_by_field_name("function")?;
            let t = ctx.text(f);
            if t.contains("GetRequiredService<") || t.contains("GetService<") {
                generic_args(t).into_iter().next()
            } else {
                None
            }
        }
        _ => None,
    }
}

fn local_decl(ctx: &mut Ctx, vd: Node) {
    let ty = vd.child_by_field_name("type").map(|t| ctx.text(t).to_string()).filter(|t| t != "var");
    for d in named_children(vd) {
        if d.kind() != "variable_declarator" {
            continue;
        }
        // `var (a, b) = Compute();`
        if let Some(tp) = named_children(d).into_iter().find(|c| c.kind() == "tuple_pattern") {
            declare_pattern(ctx, tp);
            if let Some(v) = named_children(d).into_iter().rev().find(|c| *c != tp) {
                let v = if v.kind() == "equals_value_clause" { v.named_child(0).unwrap_or(v) } else { v };
                walk(ctx, v);
            }
            continue;
        }
        let Some(n) = declarator_name(d) else { continue };
        let name = ctx.text(n).to_string();
        let value = declarator_value(d);
        let inferred = ty.clone().or_else(|| value.and_then(|v| infer_expr_type(ctx, v)));
        ctx.declare_local(&name, inferred);
        if let Some(v) = value {
            // `var g = app.MapGroup("/api/orders")`
            if v.kind() == "invocation_expression" {
                if let (Some(f), Some(args)) = (v.child_by_field_name("function"), v.child_by_field_name("arguments")) {
                    if f.kind() == "member_access_expression" {
                        let mname = f.child_by_field_name("name").map(|x| ctx.text(x)).unwrap_or("");
                        if mname == "MapGroup" {
                            if let Some((p, _)) = first_string_arg(args, ctx.src) {
                                let base = f
                                    .child_by_field_name("expression")
                                    .and_then(|e| route_group_prefix(ctx, ctx.text(e)))
                                    .unwrap_or_default();
                                let full = join_route(&base, &p);
                                if let Some(s) = ctx.scopes.last_mut() {
                                    s.route_groups.insert(name.clone(), full);
                                }
                            }
                        }
                    }
                }
            }
            walk(ctx, v);
        }
    }
}

/// Declares every name bound by a (possibly nested) tuple / positional pattern.
fn declare_pattern(ctx: &mut Ctx, node: Node) {
    for ch in named_children(node) {
        match ch.kind() {
            "identifier" => {
                let n = ctx.text(ch).to_string();
                ctx.declare_local(&n, None);
            }
            _ => declare_pattern(ctx, ch),
        }
    }
}

fn route_group_prefix(ctx: &Ctx, var: &str) -> Option<String> {
    ctx.scopes.iter().rev().find_map(|s| s.route_groups.get(var).cloned())
}

fn receiver_info(ctx: &Ctx, r: Node) -> (String, Option<String>) {
    let text = simplify_receiver(ctx.text(r));
    let ty = match r.kind() {
        "identifier" => ctx.var_type(&text),
        "this_expression" | "this" => ctx.class_name(),
        "member_access_expression" => {
            let e = r.child_by_field_name("expression");
            let n = r.child_by_field_name("name").map(|n| ctx.text(n).to_string());
            match (e, n) {
                (Some(e), Some(n)) if ctx.text(e) == "this" => ctx.field_type(&n),
                _ => None,
            }
        }
        "object_creation_expression" => r.child_by_field_name("type").map(|t| ctx.text(t).to_string()),
        _ => None,
    };
    let ty = if text == "this" { ctx.class_name() } else { ty };
    (text, ty)
}

fn call(ctx: &mut Ctx, node: Node) {
    let Some(func) = node.child_by_field_name("function") else {
        walk_children(ctx, node);
        return;
    };
    let args = node.child_by_field_name("arguments");
    let argc = args.map(count_args);
    let line = line_of(node);
    // `nameof(X)` is a compile-time string, not a call nor a use of X as a value.
    if func.kind() == "identifier" && ctx.text(func) == "nameof" {
        return;
    }
    let (name, recv_node, generic): (String, Option<Node>, Option<String>) = match func.kind() {
        "identifier" => (ctx.text(func).to_string(), None, None),
        // `a?.M()`: the receiver is the condition, the member sits in the binding expression.
        "conditional_access_expression" => {
            let binding = named_children(func).into_iter().find(|c| c.kind() == "member_binding_expression");
            let n = binding.and_then(|b| b.child_by_field_name("name"));
            let (nm, g) = match n {
                Some(n) if n.kind() == "generic_name" => {
                    let id = named_children(n).into_iter().find(|c| c.kind() == "identifier");
                    let ta = named_children(n).into_iter().find(|c| c.kind() == "type_argument_list");
                    (id.map(|i| ctx.text(i).to_string()).unwrap_or_default(), ta.map(|t| ctx.text(t).to_string()))
                }
                Some(n) => (ctx.text(n).to_string(), None),
                None => (last_segment(ctx.text(func)).to_string(), None),
            };
            (nm, func.child_by_field_name("condition"), g)
        }
        "generic_name" => {
            let id = named_children(func).into_iter().find(|c| c.kind() == "identifier");
            let ta = named_children(func).into_iter().find(|c| c.kind() == "type_argument_list");
            (id.map(|i| ctx.text(i).to_string()).unwrap_or_default(), None, ta.map(|t| ctx.text(t).to_string()))
        }
        "member_access_expression" => {
            let n = func.child_by_field_name("name");
            let (nm, g) = match n {
                Some(n) if n.kind() == "generic_name" => {
                    let id = named_children(n).into_iter().find(|c| c.kind() == "identifier");
                    let ta = named_children(n).into_iter().find(|c| c.kind() == "type_argument_list");
                    (id.map(|i| ctx.text(i).to_string()).unwrap_or_default(), ta.map(|t| ctx.text(t).to_string()))
                }
                Some(n) => (ctx.text(n).to_string(), None),
                None => (String::new(), None),
            };
            (nm, func.child_by_field_name("expression"), g)
        }
        _ => (last_segment(ctx.text(func)).to_string(), None, None),
    };
    let (receiver, rtype) = match recv_node {
        Some(r) => {
            let (t, ty) = receiver_info(ctx, r);
            (Some(t), ty)
        }
        None => (None, None),
    };

    let mut skip_args: Vec<Node> = Vec::new();
    special_call(ctx, &name, receiver.as_deref(), rtype.as_deref(), generic.as_deref(), args, line, &mut skip_args);

    ctx.push_ref(RefKind::Call, &name, receiver.clone(), rtype, argc, line, generic.map(|g| collapse_ws(&g)));
    if let Some(r) = recv_node {
        walk(ctx, r);
    }
    if let Some(a) = args {
        for arg in named_children(a) {
            if skip_args.contains(&arg) {
                continue;
            }
            walk(ctx, arg);
        }
    }
}

fn arg_nodes(args: Option<Node>) -> Vec<Node> {
    let Some(a) = args else { return Vec::new() };
    named_children(a).into_iter().map(|arg| if arg.kind() == "argument" { arg.named_child(0).unwrap_or(arg) } else { arg }).collect()
}

#[allow(clippy::too_many_arguments)]
fn special_call<'t>(
    ctx: &mut Ctx,
    name: &str,
    receiver: Option<&str>,
    rtype: Option<&str>,
    generic: Option<&str>,
    args: Option<Node<'t>>,
    line: u32,
    skip_args: &mut Vec<Node<'t>>,
) {
    let recv_lower = receiver.unwrap_or("").to_ascii_lowercase();
    let first_str = args.and_then(|a| first_string_arg(a, ctx.src)).map(|(s, _)| s);
    match name {
        // ---- Dependency injection ----
        "AddScoped" | "AddSingleton" | "AddTransient" | "TryAddScoped" | "TryAddSingleton" | "TryAddTransient" | "AddKeyedScoped"
        | "AddKeyedSingleton" | "AddKeyedTransient" | "AddHostedService" | "AddDbContext" | "AddDbContextPool" | "AddHttpClient" => {
            let lifetime = if name.contains("Scoped") || name.starts_with("AddDbContext") {
                "scoped"
            } else if name.contains("Singleton") || name == "AddHostedService" {
                "singleton"
            } else {
                "transient"
            };
            let gargs = generic.map(generic_args).unwrap_or_default();
            let anodes = arg_nodes(args);
            let typeofs: Vec<String> = anodes
                .iter()
                .filter(|n| n.kind() == "typeof_expression")
                .filter_map(|n| n.child_by_field_name("type").or_else(|| n.named_child(0)))
                .map(|t| ctx.text(t).to_string())
                .collect();
            let (service, implementation) = if gargs.len() >= 2 {
                (gargs[0].clone(), Some(gargs[1].clone()))
            } else if gargs.len() == 1 {
                // Factory registrations: AddScoped<IFoo>(sp => new Foo(...)).
                let factory_impl = anodes.iter().find_map(|n| find_creation_type(ctx, *n));
                (gargs[0].clone(), factory_impl.or_else(|| Some(gargs[0].clone())))
            } else if typeofs.len() >= 2 {
                (typeofs[0].clone(), Some(typeofs[1].clone()))
            } else if typeofs.len() == 1 {
                (typeofs[0].clone(), Some(typeofs[0].clone()))
            } else {
                return;
            };
            ctx.push_di(&clean_type(&service), implementation.map(|i| clean_type(&i)).as_deref(), lifetime, "aspnet", line);
        }
        // ---- Minimal API routes ----
        "MapGet" | "MapPost" | "MapPut" | "MapDelete" | "MapPatch" | "Map" | "MapMethods" => {
            let Some(path) = first_str.clone() else { return };
            let method = match name {
                "MapGet" => "GET",
                "MapPost" => "POST",
                "MapPut" => "PUT",
                "MapDelete" => "DELETE",
                "MapPatch" => "PATCH",
                _ => "ANY",
            };
            let prefix = receiver.and_then(|r| route_group_prefix(ctx, r)).unwrap_or_default();
            let full = join_route(&prefix, &path);
            let anodes = arg_nodes(args);
            let handler = anodes.last().copied();
            match handler {
                Some(h) if matches!(h.kind(), "lambda_expression" | "anonymous_method_expression") => {
                    let label = format!("{method} {full}");
                    let mut s = NewSymbol::new(SymbolKind::Function, &label, ctx.child_qname(&label), line_of(h), end_line_of(h));
                    s.signature = format!("{label} => {}", truncate(&collapse_ws(ctx.text(h)), 80));
                    s.is_entrypoint = true;
                    s.entry_kind = Some(format!("route {method} {full}"));
                    s.complexity = cyclomatic(h, ctx.src, ctx.lang);
                    s.body_hash = token_hash(h, ctx.src);
                    s.is_async = ctx.text(h).trim_start().starts_with("async");
                    let id = ctx.add_symbol(s);
                    ctx.push_route(&id, method, &full, "aspnet-minimal", line);
                    ctx.push_scope(Scope {
                        symbol_id: id,
                        kind: Some(SymbolKind::Function),
                        qname: ctx.child_qname(&label),
                        is_function: true,
                        ..Default::default()
                    });
                    walk(ctx, h);
                    ctx.pop_scope();
                    // Keep the literal path recorded.
                    if let Some(a) = args {
                        if let Some((_, n)) = first_string_arg(a, ctx.src) {
                            let v = string_value(n, ctx.src);
                            ctx.push_literal(&v, "string", n);
                        }
                    }
                    for a in named_children(args.unwrap()) {
                        skip_args.push(a);
                    }
                }
                Some(h) if matches!(h.kind(), "identifier" | "member_access_expression") => {
                    let target = last_segment(ctx.text(h)).to_string();
                    let enclosing = ctx.enclosing();
                    ctx.push_route(&format!("ref:{target}|{enclosing}"), method, &full, "aspnet-minimal", line);
                }
                _ => {
                    let enclosing = ctx.enclosing();
                    ctx.push_route(&enclosing, method, &full, "aspnet-minimal", line);
                }
            }
        }
        // ---- Configuration ----
        "GetConnectionString" => {
            if let Some(k) = first_str.clone() {
                ctx.push_config(&format!("ConnectionStrings:{k}"), "appsettings", line, None);
            }
        }
        "GetValue" | "GetSection" | "GetRequiredSection" if recv_lower.contains("config") => {
            if let Some(k) = first_str.clone() {
                let default = arg_nodes(args).get(1).map(|n| ctx.text(*n).to_string());
                ctx.push_config(&k, "appsettings", line, default);
            }
        }
        "GetEnvironmentVariable" => {
            if let Some(k) = first_str.clone() {
                ctx.push_config(&k, "env", line, None);
            }
        }
        // ---- HTTP clients ----
        "GetAsync"
        | "PostAsync"
        | "PutAsync"
        | "DeleteAsync"
        | "PatchAsync"
        | "GetFromJsonAsync"
        | "PostAsJsonAsync"
        | "PutAsJsonAsync"
        | "PatchAsJsonAsync"
        | "GetStringAsync"
        | "GetStreamAsync"
        | "GetByteArrayAsync"
        | "DeleteFromJsonAsync"
            if recv_lower.contains("http") || recv_lower.contains("client") || rtype.map(|t| t.contains("HttpClient")).unwrap_or(false) =>
        {
            if let Some(url) = first_str.clone() {
                let m = ["Get", "Post", "Put", "Delete", "Patch"]
                    .iter()
                    .find(|p| name.starts_with(*p))
                    .map(|s| s.to_ascii_uppercase())
                    .unwrap_or_else(|| "GET".into());
                ctx.push_http_call(&m, &url, "HttpClient", line);
            }
        }
        "UseExceptionHandler" | "UseDeveloperExceptionPage" => {
            let enclosing = ctx.enclosing();
            ctx.facts.global_handlers.push(GlobalHandler { handler: enclosing, exception: "*".into(), framework: "aspnet".into(), line });
        }
        "Run" | "RunAsync" | "Start" if recv_lower == "app" || recv_lower.ends_with("host") => {
            let enclosing = ctx.enclosing();
            if let Some(s) = ctx.symbol_mut(&enclosing) {
                if !s.is_entrypoint {
                    s.is_entrypoint = true;
                    s.entry_kind = Some("server start".into());
                }
            }
        }
        _ => {}
    }

    // ---- State mutations ----
    let prefs = recv_lower.ends_with("playerprefs") || recv_lower.ends_with("editorprefs");
    match name {
        "SetInt" | "SetFloat" | "SetString" | "SetBool" | "DeleteKey" | "DeleteAll" if prefs => {
            let store = if recv_lower.ends_with("editorprefs") { "EditorPrefs" } else { "PlayerPrefs" };
            let key = first_str.clone().unwrap_or_else(|| "*".into());
            ctx.push_mutation(&format!("{store}:{key}"), "storage", line);
        }
        "Get" | "Post" | "Put" | "Delete" | "Head" | "PostWwwForm" if recv_lower.ends_with("unitywebrequest") => {
            if let Some(url) = first_str.clone() {
                ctx.push_http_call(name.trim_end_matches("WwwForm"), &url, "UnityWebRequest", line);
            }
        }
        "SaveChanges" | "SaveChangesAsync" => {
            let target = rtype.map(|t| t.to_string()).unwrap_or_else(|| receiver.unwrap_or("DbContext").to_string());
            ctx.push_mutation(&format!("{target}.SaveChanges"), "database", line);
        }
        "Add" | "AddAsync" | "AddRange" | "AddRangeAsync" | "Update" | "UpdateRange" | "Remove" | "RemoveRange" | "Attach"
        | "ExecuteDelete" | "ExecuteDeleteAsync" | "ExecuteUpdate" | "ExecuteUpdateAsync" => {
            if let Some(r) = receiver {
                let segs: Vec<&str> = r.split('.').collect();
                let dbish = |s: &str| {
                    let l = s.trim_start_matches('_').to_ascii_lowercase();
                    l.contains("db") || l.contains("context") || l == "ctx"
                };
                if segs.len() >= 2 && segs[..segs.len() - 1].iter().any(|s| dbish(s)) {
                    ctx.push_mutation(segs[segs.len() - 1], "database", line);
                } else if rtype.map(|t| t.contains("DbSet")).unwrap_or(false) {
                    let ent = generic_args(rtype.unwrap()).into_iter().next().unwrap_or_else(|| r.to_string());
                    ctx.push_mutation(&ent, "database", line);
                } else if segs.len() == 1 && (dbish(segs[0]) || rtype.map(|t| t.contains("Context")).unwrap_or(false)) {
                    let ent = arg_nodes(args)
                        .first()
                        .filter(|n| n.kind() == "identifier")
                        .and_then(|n| ctx.var_type(ctx.text(*n)))
                        .map(|t| clean_type(&t))
                        .unwrap_or_else(|| "entity".into());
                    ctx.push_mutation(&ent, "database", line);
                } else if recv_lower.contains("cache") {
                    ctx.push_mutation(r, "cache", line);
                }
            }
        }
        "Set" | "SetAsync" | "SetString" | "SetStringAsync" | "RemoveAsync" | "Refresh" | "RefreshAsync" | "GetOrCreate"
        | "GetOrCreateAsync" | "StringSet" | "StringSetAsync" | "KeyDelete" | "KeyDeleteAsync"
            if recv_lower.contains("cache")
                || recv_lower.contains("redis")
                || rtype.map(|t| t.contains("Cache") || t.contains("IDatabase")).unwrap_or(false) =>
        {
            let target = first_str.map(|k| format!("cache:{k}")).unwrap_or_else(|| receiver.unwrap_or("cache").to_string());
            ctx.push_mutation(&target, "cache", line);
        }
        _ => {}
    }
}

fn find_creation_type(ctx: &Ctx, node: Node) -> Option<String> {
    if node.kind() == "object_creation_expression" {
        return node.child_by_field_name("type").map(|t| ctx.text(t).to_string());
    }
    let mut c = node.walk();
    let children: Vec<Node> = node.named_children(&mut c).collect();
    children.into_iter().find_map(|ch| find_creation_type(ctx, ch))
}

fn new_expr(ctx: &mut Ctx, node: Node) {
    let ty = node.child_by_field_name("type").map(|t| ctx.text(t).to_string()).unwrap_or_default();
    let args = node.child_by_field_name("arguments");
    let argc = args.map(count_args).or(Some(0));
    let generic = if ty.contains('<') { Some(ty.clone()) } else { None };
    ctx.push_ref(RefKind::New, &clean_type(&ty), None, None, argc, line_of(node), generic);
    if let Some(a) = args {
        walk(ctx, a);
    }
    if let Some(init) = node.child_by_field_name("initializer") {
        initializer(ctx, init, &ty);
    }
}

/// `new Order { Name = n, Lines = { … } }`: member assignments write members of the created
/// type, never fields of the enclosing class that happen to share the name.
fn initializer(ctx: &mut Ctx, init: Node, ty: &str) {
    let owner = clean_type(ty);
    for ch in named_children(init) {
        if ch.kind() == "assignment_expression" && !owner.is_empty() {
            if let Some(left) = ch.child_by_field_name("left").filter(|l| l.kind() == "identifier") {
                let name = ctx.text(left).to_string();
                let line = line_of(left);
                ctx.push_ref(RefKind::Write, &name, Some(owner.clone()), Some(owner.clone()), None, line, None);
                ctx.push_mutation(&format!("{owner}.{name}"), "object_field", line);
                if let Some(right) = ch.child_by_field_name("right") {
                    walk(ctx, right);
                }
                continue;
            }
        }
        walk(ctx, ch);
    }
}

fn throw(ctx: &mut Ctx, node: Node) {
    let Some(expr) = node.named_child(0) else { return };
    match expr.kind() {
        "object_creation_expression" => {
            let ty = expr.child_by_field_name("type").map(|t| clean_type(ctx.text(t))).unwrap_or_default();
            let args = expr.child_by_field_name("arguments");
            let msg = args.and_then(|a| first_string_arg(a, ctx.src)).map(|(s, _)| s);
            ctx.push_ref(RefKind::Throw, &ty, None, None, None, line_of(node), msg);
            if let Some(a) = args {
                walk(ctx, a);
            }
        }
        "identifier" => {
            let n = ctx.text(expr).to_string();
            if let Some(t) = ctx.var_type(&n) {
                ctx.push_ref(RefKind::Throw, &clean_type(&t), None, None, None, line_of(node), None);
            }
        }
        _ => walk(ctx, expr),
    }
}

fn try_stmt(ctx: &mut Ctx, node: Node) {
    let mut types = Vec::new();
    let mut catches = Vec::new();
    let mut finally = None;
    for ch in named_children(node) {
        match ch.kind() {
            "catch_clause" => {
                let decl = named_children(ch).into_iter().find(|c| c.kind() == "catch_declaration");
                let ty = decl.and_then(|d| d.child_by_field_name("type")).map(|t| clean_type(ctx.text(t))).unwrap_or_else(|| "*".into());
                types.push(ty.clone());
                catches.push((ch, decl, ty));
            }
            "finally_clause" => finally = Some(ch),
            _ => {}
        }
    }
    ctx.push_catch_frame(types);
    if let Some(b) = node.child_by_field_name("body") {
        walk(ctx, b);
    }
    ctx.pop_catch_frame();
    for (clause, decl, ty) in catches {
        ctx.push_ref(RefKind::Catch, &ty, None, None, None, line_of(clause), None);
        if let Some(d) = decl {
            if let Some(n) = d.child_by_field_name("name") {
                let name = ctx.text(n).to_string();
                ctx.declare_local(&name, Some(ty.clone()));
            }
        }
        for ch in named_children(clause) {
            if matches!(ch.kind(), "block" | "catch_filter_clause") {
                walk(ctx, ch);
            }
        }
    }
    if let Some(f) = finally {
        walk_children(ctx, f);
    }
}

fn write_target(ctx: &mut Ctx, left: Node) {
    let line = line_of(left);
    match left.kind() {
        "identifier" => {
            let name = ctx.text(left).to_string();
            if ctx.is_local(&name) {
                return;
            }
            if ctx.is_field(&name) {
                let cls = ctx.class_name();
                ctx.push_ref(RefKind::Write, &name, Some("this".into()), cls.clone(), None, line, None);
                let q = format!("{}.{}", cls.unwrap_or_default(), name);
                ctx.push_mutation(&q, "local_field", line);
            } else if ctx.is_module_var(&name) {
                if ctx.in_function() {
                    ctx.push_ref(RefKind::Write, &name, None, None, None, line, None);
                    ctx.push_mutation(&name, "global_var", line);
                }
            } else {
                ctx.push_ref(RefKind::Write, &name, None, None, None, line, None);
            }
        }
        "member_access_expression" => {
            let expr = left.child_by_field_name("expression");
            let name = left.child_by_field_name("name").map(|n| ctx.text(n).to_string()).unwrap_or_default();
            match expr {
                Some(e) if ctx.text(e) == "this" => {
                    let cls = ctx.class_name();
                    ctx.push_ref(RefKind::Write, &name, Some("this".into()), cls.clone(), None, line, None);
                    ctx.push_mutation(&format!("{}.{}", cls.unwrap_or_default(), name), "local_field", line);
                }
                Some(e) => {
                    let (rtext, rtype) = receiver_info(ctx, e);
                    let owner = rtype.clone().map(|t| clean_type(&t)).unwrap_or_else(|| rtext.clone());
                    ctx.push_ref(RefKind::Write, &name, Some(rtext), rtype, None, line, None);
                    ctx.push_mutation(&format!("{owner}.{name}"), "object_field", line);
                    walk(ctx, e);
                }
                None => {}
            }
        }
        "tuple_expression" => {
            for a in named_children(left) {
                let target = if a.kind() == "argument" { a.named_child(0).unwrap_or(a) } else { a };
                write_target(ctx, target);
            }
        }
        _ => walk(ctx, left),
    }
}

fn element_access(ctx: &mut Ctx, node: Node) {
    let Some(expr) = node.child_by_field_name("expression") else { return };
    let et = ctx.text(expr).to_ascii_lowercase();
    let last = last_segment(&et).trim_start_matches('_').to_string();
    if !(last.contains("config") || last == "configuration") {
        return;
    }
    let Some(sub) = node.child_by_field_name("subscript") else { return };
    if let Some((k, _)) = first_string_arg(sub, ctx.src) {
        let default = node.parent().and_then(|p| {
            if p.kind() == "binary_expression" && p.child_by_field_name("left") == Some(node) {
                let op = p.child_by_field_name("operator").map(|o| ctx.text(o)).unwrap_or("");
                if op == "??" {
                    return p.child_by_field_name("right").map(|r| ctx.text(r).to_string());
                }
            }
            None
        });
        ctx.push_config(&k, "appsettings", line_of(node), default);
    }
}
