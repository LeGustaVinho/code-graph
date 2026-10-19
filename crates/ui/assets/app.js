/* CodeGraph Explorer — application logic. */
(() => {
  "use strict";

  // ------------------------------------------------------------------ helpers
  const t = (k, v) => I18N.t(k, v);
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];
  const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  const cssVar = (v) => getComputedStyle(document.documentElement).getPropertyValue(v).trim();
  const num = (n) => new Intl.NumberFormat(I18N.lang).format(n ?? 0);
  const store = {
    get(k, d) {
      try {
        const v = localStorage.getItem("codegraph." + k);
        return v === null ? d : JSON.parse(v);
      } catch {
        return d;
      }
    },
    set(k, v) {
      try {
        localStorage.setItem("codegraph." + k, JSON.stringify(v));
      } catch {}
    },
  };
  /** Replaces innerHTML only when it changed (keeps focus/hover stable during polling). */
  function setHTML(el, html) {
    if (el._html === html) return;
    el._html = html;
    el.innerHTML = html;
  }
  function relTime(ms) {
    if (!ms) return "—";
    const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
    if (s < 10) return t("time.now");
    if (s < 60) return t("time.s", { n: s });
    if (s < 3600) return t("time.m", { n: Math.round(s / 60) });
    if (s < 86400) return t("time.h", { n: Math.round(s / 3600) });
    return t("time.d", { n: Math.round(s / 86400) });
  }

  // ------------------------------------------------------------------ constants
  const KINDS = {
    class: { color: "#6366f1", shape: "round-rectangle" },
    interface: { color: "#8b5cf6", shape: "diamond" },
    struct: { color: "#6366f1", shape: "round-rectangle" },
    enum: { color: "#ec4899", shape: "hexagon" },
    type_alias: { color: "#a855f7", shape: "round-rectangle" },
    data_model: { color: "#f97316", shape: "barrel" },
    method: { color: "#10b981", shape: "ellipse" },
    constructor: { color: "#059669", shape: "ellipse" },
    function: { color: "#14b8a6", shape: "ellipse" },
    test: { color: "#f59e0b", shape: "tag" },
    property: { color: "#94a3b8", shape: "ellipse" },
    field: { color: "#94a3b8", shape: "ellipse" },
    variable: { color: "#94a3b8", shape: "ellipse" },
    const: { color: "#c084fc", shape: "pentagon" },
    module: { color: "#64748b", shape: "rectangle" },
    literal: { color: "#eab308", shape: "tag" },
    unresolved: { color: "#ef4444", shape: "octagon" },
  };
  const kindColor = (k) => (KINDS[k] || KINDS.variable).color;
  const RELATIONS = ["CALLS", "HTTP_CALLS", "RESOLVES_DI", "INHERITS", "THROWS", "CAUGHT_BY", "TESTS", "IMPORTS", "READS_VAR", "WRITES_VAR", "USES_LITERAL"];
  const REL_COLORS = {
    CALLS: "#64748b", HTTP_CALLS: "#6366f1", RESOLVES_DI: "#8b5cf6", INHERITS: "#0ea5e9", THROWS: "#ef4444", CAUGHT_BY: "#f97316",
    TESTS: "#10b981", IMPORTS: "#94a3b8", READS_VAR: "#14b8a6", WRITES_VAR: "#eab308", USES_LITERAL: "#a3a3a3",
  };
  const DEFAULT_RELATIONS = ["CALLS", "HTTP_CALLS", "RESOLVES_DI", "INHERITS", "THROWS", "CAUGHT_BY", "TESTS"];
  const CONFIDENCES = ["exact", "inferred", "unresolved"];
  const LAYOUTS = ["auto", "force", "hierarchy", "concentric", "circle", "grid"];
  const VIEWS = [
    { id: "dashboard", icon: "dashboard" },
    { id: "graph", icon: "graph" },
    { id: "entrypoints", icon: "entry" },
    { id: "models", icon: "database" },
    { id: "hotspots", icon: "flame" },
    { id: "unresolved", icon: "alert" },
  ];
  const TABS = [
    { id: "overview", icon: "info" },
    { id: "evidence", icon: "link" },
    { id: "flows", icon: "activity" },
    { id: "data", icon: "table" },
    { id: "source", icon: "code" },
    { id: "actions", icon: "zap" },
  ];

  const state = {
    relations: new Set(store.get("relations", DEFAULT_RELATIONS).filter((r) => RELATIONS.includes(r))),
    conf: new Set(store.get("confidence", CONFIDENCES)),
    layout: store.get("layout", "auto"),
    view: "dashboard",
    mode: "dashboard",
    focus: null,
    depth: 1,
    history: [],
    selected: null,
    detail: null,
    tab: store.get("tab", "overview"),
    evFilter: "all",
    overview: null,
    status: null,
    lastIndexed: null,
    listFilter: "",
    cache: {},
  };

  // ------------------------------------------------------------------ API
  async function api(path, opts) {
    const res = await fetch(path, opts);
    const body = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(body.error || res.statusText);
    return body;
  }
  const tool = (name, args) => api(`/api/tool/${name}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(args) });

  let loading = 0;
  function busy(on) {
    loading += on ? 1 : -1;
    $("#progress").hidden = loading <= 0;
  }

  function toast(msg, kind = "") {
    const el = document.createElement("div");
    el.className = `toast ${kind}`;
    el.innerHTML = `${icon(kind === "error" ? "alert" : "check", 15)}<span>${esc(msg)}</span>`;
    $("#toasts").appendChild(el);
    setTimeout(() => el.remove(), 2600);
  }

  async function copy(text, msg) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      ta.remove();
    }
    toast(msg || t("insp.copied"));
  }

  // ------------------------------------------------------------------ theme & language
  function effectiveTheme() {
    const forced = document.documentElement.dataset.theme;
    if (forced) return forced;
    return matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }
  function renderThemeIcon() {
    $("#theme-icon").innerHTML = icon(effectiveTheme() === "dark" ? "sun" : "moon", 17);
  }
  function toggleTheme() {
    const next = effectiveTheme() === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem("codegraph.theme", next);
    } catch {}
    renderThemeIcon();
    cy.style(buildStyle());
    if (state.mode === "dashboard") renderInspector();
  }
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    renderThemeIcon();
    cy.style(buildStyle());
  });

  function renderLangSelect() {
    const sel = $("#lang-select");
    sel.innerHTML = Object.entries(I18N.langs)
      .map(([code, l]) => `<option value="${code}" ${code === I18N.lang ? "selected" : ""}>${l.name}</option>`)
      .join("");
  }
  function setLanguage(code) {
    I18N.setLang(code);
    state.cache = {};
    renderLangSelect();
    renderChrome();
    renderInspector();
    cy.edges().forEach((e) => e.data("rlabel", t("rel." + e.data("relation"))));
  }

  // ------------------------------------------------------------------ chrome (sidebar, stage bar)
  function renderChrome() {
    I18N.apply();
    renderLayoutSelect();
    renderSidebar();
    renderStageBar();
    renderLegend();
    renderStatus();
    $("#kbd-hint").textContent = /mac/i.test(navigator.platform) ? "⌘ K" : "Ctrl K";
  }

  function viewCount(id) {
    const o = state.overview;
    if (!o) return "";
    switch (id) {
      case "entrypoints": return o.entrypoints.length;
      case "models": return o.models.length;
      case "hotspots": return o.hotspots.length;
      case "unresolved": return o.unresolved.length;
      default: return "";
    }
  }

  function renderSidebar() {
    setHTML($("#nav-views"), VIEWS.map((v) => {
      const c = viewCount(v.id);
      return `<button class="nav-item ${state.view === v.id ? "active" : ""}" data-view="${v.id}">${icon(v.icon)}<span class="label">${esc(t("view." + v.id))}</span>${c !== "" ? `<span class="count">${num(c)}</span>` : ""}</button>`;
    }).join(""));

    const relCounts = Object.fromEntries((state.overview?.relations || []).map(([r, c]) => [r, c]));
    const confCounts = {};
    for (const rc of state.overview?.relation_confidence || []) confCounts[rc.confidence] = (confCounts[rc.confidence] || 0) + rc.count;
    const check = icon("check", 11);
    setHTML($("#conf-filters"), CONFIDENCES.map(
      (c) => `<button class="filter ${state.conf.has(c) ? "" : "off"}" data-conf="${c}" title="${esc(t(`conf.${c}.desc`))}">
        <span class="checkbox">${check}</span><span class="line-swatch ${c}"></span><span class="label">${esc(t("conf." + c))}</span><span class="count">${num(confCounts[c] || 0)}</span></button>`
    ).join(""));
    setHTML($("#rel-filters"), RELATIONS.map(
      (r) => `<button class="filter ${state.relations.has(r) ? "" : "off"}" data-rel="${r}">
        <span class="checkbox">${check}</span><span class="swatch" style="background:${REL_COLORS[r]}"></span><span class="label">${esc(t("rel." + r))}</span><span class="count">${num(relCounts[r] || 0)}</span></button>`
    ).join(""));

    const st = state.status?.stats;
    setHTML($("#index-info"), st
      ? `<span>${esc(t("index.files"))}</span><b>${num(st.files)}</b>
         <span>${esc(t("index.nodes"))}</span><b>${num(st.nodes)}</b>
         <span>${esc(t("index.edges"))}</span><b>${num(st.edges)}</b>
         <span>${esc(t("index.last"))}</span><b>${esc(relTime(st.last_indexed_at))}</b>`
      : "");
  }

  function renderLayoutSelect() {
    $("#layout-select").innerHTML = LAYOUTS.map((l) => {
      const label = l === "auto" ? "Auto" : t("layout." + l);
      return `<option value="${l}" ${state.layout === l ? "selected" : ""}>${esc(label)}</option>`;
    }).join("");
  }

  function renderStageBar() {
    const crumbs = [`<button class="crumb ${state.focus ? "" : "current"}" data-crumb="home">${esc(t("stage.home"))}</button>`];
    const trail = state.history.slice(-4);
    trail.forEach((h, i) => {
      crumbs.push(`<span class="crumb-sep">${icon("chevron", 13)}</span>`);
      const last = i === trail.length - 1;
      crumbs.push(`<button class="crumb ${last ? "current" : ""}" data-crumb="${esc(h.id)}" title="${esc(h.label)}">${esc(h.label)}</button>`);
    });
    $("#crumbs").innerHTML = crumbs.join("");
    $("#btn-back").disabled = state.history.length === 0;
    $("#depth-ctl").hidden = !state.focus;
    $$("#depth-ctl button").forEach((b) => b.classList.toggle("on", Number(b.dataset.depth) === state.depth));
    const n = cy.nodes(":visible").length;
    const e = cy.edges(":visible").length;
    $("#stage-count").textContent = n ? t("stage.nodes", { n: num(n), e: num(e) }) : "";
  }

  function renderLegend() {
    const present = new Set(cy.nodes().map((n) => n.data("kind")));
    const kinds = Object.keys(KINDS).filter((k) => present.has(k));
    const items = kinds.map((k) => `<span class="legend-item"><span class="swatch" style="background:${KINDS[k].color}"></span>${esc(t("kind." + k))}</span>`);
    items.push('<span class="legend-sep"></span>');
    for (const c of CONFIDENCES) items.push(`<span class="legend-item"><span class="line-swatch ${c}"></span>${esc(t("conf." + c))}</span>`);
    $("#legend-body").innerHTML = items.join("");
    $("#legend").classList.toggle("collapsed", store.get("legendCollapsed", false));
  }

  function renderStatus() {
    const s = state.status;
    const pill = $("#status");
    const text = pill.querySelector(".status-text");
    if (!s) {
      pill.className = "status-pill bad";
      text.textContent = t("status.offline");
      return;
    }
    if (s.reindexing) {
      pill.className = "status-pill busy";
      text.textContent = t("status.reindexing", { n: s.pending_files || "" });
    } else if (s.stale_files.length) {
      pill.className = "status-pill stale";
      text.textContent = t("status.pending", { n: s.stale_files.length });
    } else if (s.stats.error_files) {
      pill.className = "status-pill bad";
      text.textContent = t("status.errors", { n: s.stats.error_files });
    } else {
      pill.className = "status-pill ok";
      text.textContent = t("status.ok");
    }
    pill.title = s.watching ? t("status.watching") : t("status.polling");
    $("#project-name").textContent = s.root;
    $("#project-name").title = s.root;
  }

  // ------------------------------------------------------------------ graph
  function buildStyle() {
    const text = cssVar("--text");
    const bg = cssVar("--canvas");
    const surface = cssVar("--surface");
    const kindStyles = Object.entries(KINDS).map(([k, v]) => ({ selector: `node[kind = '${k}']`, style: { "background-color": v.color, shape: v.shape } }));
    return [
      {
        selector: "node",
        style: {
          "background-color": "#94a3b8",
          width: "mapData(deg, 0, 14, 16, 38)",
          height: "mapData(deg, 0, 14, 16, 38)",
          label: "data(label)",
          "font-family": cssVar("--sans"),
          "font-size": 11,
          "font-weight": 500,
          color: text,
          "text-valign": "bottom",
          "text-margin-y": 5,
          "text-wrap": "ellipsis",
          "text-max-width": 170,
          "text-background-color": bg,
          "text-background-opacity": 0.85,
          "text-background-padding": 2,
          "text-background-shape": "roundrectangle",
          "min-zoomed-font-size": 7,
          "border-width": 2,
          "border-color": surface,
        },
      },
      ...kindStyles,
      { selector: "node[?is_entrypoint]", style: { "border-color": "#f59e0b", "border-width": 3 } },
      { selector: "node[kind = 'unresolved']", style: { "background-opacity": 0.12, "border-style": "dashed", "border-color": cssVar("--unresolved"), color: cssVar("--unresolved"), width: 18, height: 18 } },
      { selector: "node.focus", style: { "border-color": cssVar("--accent"), "border-width": 4, "font-weight": 700, "font-size": 12.5, "z-index": 10 } },
      { selector: "node:selected", style: { "overlay-color": cssVar("--accent"), "overlay-opacity": 0.14, "overlay-padding": 7 } },
      { selector: "node.hover", style: { "border-color": cssVar("--accent"), "border-width": 3, "z-index": 9 } },
      { selector: "node.dim", style: { opacity: 0.18 } },
      {
        selector: "edge",
        style: {
          width: 1.6,
          "curve-style": "bezier",
          "line-color": cssVar("--exact"),
          "target-arrow-color": cssVar("--exact"),
          "target-arrow-shape": "triangle",
          "arrow-scale": 0.75,
          opacity: 0.55,
          "font-size": 9.5,
          "font-family": cssVar("--sans"),
          color: cssVar("--muted"),
          "text-rotation": "autorotate",
          "text-background-color": bg,
          "text-background-opacity": 0.9,
          "text-background-padding": 2,
          "min-zoomed-font-size": 8,
        },
      },
      { selector: "edge[confidence = 'inferred']", style: { "line-style": "dashed", "line-dash-pattern": [6, 4], "line-color": cssVar("--inferred"), "target-arrow-color": cssVar("--inferred"), opacity: 0.75 } },
      { selector: "edge[confidence = 'unresolved']", style: { "line-style": "dotted", "line-color": cssVar("--unresolved"), "target-arrow-color": cssVar("--unresolved"), "target-arrow-shape": "tee", width: 2, opacity: 0.8 } },
      { selector: "edge[relation = 'INHERITS']", style: { "target-arrow-fill": "hollow", "arrow-scale": 1 } },
      { selector: "edge[relation = 'HTTP_CALLS']", style: { width: 2.6 } },
      { selector: "edge.hl", style: { opacity: 1, width: 2.6, label: "data(rlabel)", "z-index": 8 } },
      { selector: "edge.dim", style: { opacity: 0.06 } },
      { selector: ".conf-hidden", style: { display: "none" } },
    ];
  }

  const cy = cytoscape({ container: $("#cy"), minZoom: 0.06, maxZoom: 3, style: [] });
  cy.style(buildStyle());

  function applyConfidenceFilter() {
    cy.batch(() => {
      cy.edges().forEach((e) => e.toggleClass("conf-hidden", !state.conf.has(e.data("confidence"))));
      cy.nodes("[kind = 'unresolved']").forEach((n) => n.toggleClass("conf-hidden", !state.conf.has("unresolved")));
    });
    const anyVisible = cy.nodes(":visible").length > 0;
    $("#empty").hidden = anyVisible;
    $("#empty-body").textContent = cy.nodes().length && !anyVisible ? t("empty.filtered") : t("empty.body");
    renderStageBar();
  }

  function layoutOptions(focusId) {
    const animate = cy.nodes().length < 400;
    const base = { animate, animationDuration: 320, fit: true, padding: 48 };
    let name = state.layout;
    if (name === "auto") name = focusId ? "concentric" : "force";
    switch (name) {
      case "hierarchy":
        return { ...base, name: "breadthfirst", directed: true, spacingFactor: 1.15, roots: focusId ? cy.getElementById(focusId) : undefined };
      case "concentric":
        return {
          ...base,
          name: "concentric",
          minNodeSpacing: 70,
          spacingFactor: 1.25,
          avoidOverlap: true,
          concentric: (n) => (n.id() === focusId ? 1000 : n.degree()),
          levelWidth: () => 2,
        };
      case "circle":
        return { ...base, name: "circle", spacingFactor: 1.1 };
      case "grid":
        return { ...base, name: "grid", avoidOverlap: true };
      default:
        return { ...base, name: "cose", animate: false, nodeRepulsion: () => 9000, idealEdgeLength: () => 90, gravity: 0.3, numIter: 1500, randomize: true };
    }
  }

  function runLayout(focusId) {
    const l = cy.layout(layoutOptions(focusId));
    l.one("layoutstop", () => {
      if (cy.zoom() > 1.25) {
        cy.zoom(1.25);
        cy.center(focusId ? cy.getElementById(focusId) : undefined);
      }
    });
    l.run();
  }

  function render(data, focusId) {
    const ids = new Set(data.nodes.map((n) => n.id));
    const els = data.nodes.map((n) => ({ group: "nodes", data: n }));
    for (const e of data.edges) {
      if (ids.has(e.source) && ids.has(e.target)) els.push({ group: "edges", data: { ...e, rlabel: t("rel." + e.relation) } });
    }
    cy.batch(() => {
      cy.elements().remove();
      cy.add(els);
      cy.nodes().forEach((n) => n.data("deg", n.degree()));
    });
    if (focusId) cy.getElementById(focusId).addClass("focus");
    applyConfidenceFilter();
    runLayout(focusId);
    renderLegend();
    renderStageBar();
    if (state.selected && cy.getElementById(state.selected).length) cy.getElementById(state.selected).select();
  }

  async function loadGraph() {
    busy(true);
    try {
      const rels = [...state.relations].join(",");
      const url = state.focus
        ? `/api/graph?focus=${encodeURIComponent(state.focus)}&depth=${state.depth}&relations=${rels}`
        : `/api/graph?relations=${rels}&limit=220`;
      const data = await api(url);
      render(data, state.focus);
    } catch (e) {
      toast(e.message, "error");
    } finally {
      busy(false);
    }
  }

  function focusNode(id, label, { push = true } = {}) {
    if (push && state.focus !== id) {
      state.history.push({ id, label: label || id });
      if (state.history.length > 30) state.history.shift();
    }
    state.focus = id;
    state.view = "graph";
    renderSidebar();
    loadGraph();
    select(id);
  }

  function goHome() {
    state.focus = null;
    state.history = [];
    state.view = "graph";
    renderSidebar();
    loadGraph();
  }

  // Hover tooltip.
  const tip = $("#tip");
  function showTip(html, pos) {
    tip.innerHTML = html;
    tip.hidden = false;
    const wrap = $(".canvas-wrap").getBoundingClientRect();
    const w = tip.offsetWidth;
    const h = tip.offsetHeight;
    let x = pos.x + 14;
    let y = pos.y + 14;
    if (x + w > wrap.width - 8) x = pos.x - w - 14;
    if (y + h > wrap.height - 8) y = pos.y - h - 14;
    tip.style.left = `${Math.max(8, x)}px`;
    tip.style.top = `${Math.max(8, y)}px`;
  }
  const hideTip = () => (tip.hidden = true);
  const kindBadge = (k) => `<span class="badge"><span class="swatch" style="background:${kindColor(k)}"></span>${esc(t("kind." + k))}</span>`;

  cy.on("mouseover", "node", (ev) => {
    const n = ev.target;
    n.addClass("hover");
    const d = n.data();
    if (d.kind === "unresolved") {
      showTip(`<div class="tt-row">${kindBadge("unresolved")}</div><div class="tt-title">${esc(d.label)}</div>`, ev.renderedPosition);
      return;
    }
    showTip(
      `<div class="tt-row">${kindBadge(d.kind)}${d.is_entrypoint ? `<span class="badge entry">${esc(t("ov.entry"))}</span>` : ""}</div>
       <div class="tt-title">${esc(d.qualified_name || d.label)}</div>
       <div class="tt-mono">${esc(d.coords)}</div>
       ${d.signature ? `<div class="tt-mono" style="margin-top:4px">${esc(d.signature.slice(0, 140))}</div>` : ""}`,
      ev.renderedPosition
    );
  });
  cy.on("mouseout", "node", (ev) => {
    ev.target.removeClass("hover");
    hideTip();
  });
  cy.on("mouseover", "edge", (ev) => {
    const d = ev.target.data();
    ev.target.addClass("hl");
    showTip(
      `<div class="tt-row"><span class="conf-badge ${d.confidence}">${esc(t("conf." + d.confidence))}</span><span class="rel-tag">${esc(t("rel." + d.relation))}</span></div>
       <div class="tt-ev">${esc(d.evidence)}</div><div class="tt-mono" style="margin-top:4px">${esc(d.site)}</div>`,
      ev.renderedPosition
    );
  });
  cy.on("mouseout", "edge", (ev) => {
    if (!ev.target.connectedNodes().some((n) => n.selected())) ev.target.removeClass("hl");
    hideTip();
  });
  cy.on("pan zoom", hideTip);
  cy.on("tap", "node", (ev) => {
    const n = ev.target;
    if (n.data("kind") === "unresolved") return;
    highlightNeighbourhood(n);
    select(n.id());
  });
  cy.on("dbltap", "node", (ev) => {
    const n = ev.target;
    if (n.data("kind") !== "unresolved") focusNode(n.id(), n.data("label"));
  });
  cy.on("tap", (ev) => {
    if (ev.target === cy) cy.elements().removeClass("dim hl");
  });

  function highlightNeighbourhood(node) {
    cy.batch(() => {
      cy.elements().addClass("dim").removeClass("hl");
      const nb = node.closedNeighborhood();
      nb.removeClass("dim");
      nb.edges().addClass("hl");
    });
  }

  function exportPng() {
    const png = cy.png({ full: true, scale: 2, bg: cssVar("--canvas") });
    const a = document.createElement("a");
    a.href = png;
    a.download = "codegraph.png";
    a.click();
    toast(t("toast.exported"));
  }

  // ------------------------------------------------------------------ inspector
  const inspector = $("#inspector");

  function renderInspector() {
    if (state.mode === "node" && state.detail) return renderNode();
    if (state.mode === "list") return renderList();
    return renderDashboard();
  }

  function rowFor(n, meta = "") {
    return `<div class="row clickable" data-open="${esc(n.id)}" data-label="${esc(n.label)}">
      <span class="kind-dot" style="background:${kindColor(n.kind)}"></span>
      <div class="main"><div class="title">${esc(n.label)}</div><div class="sub">${esc(n.coords)}</div></div>
      ${meta ? `<span class="meta">${meta}</span>` : ""}</div>`;
  }

  function renderDashboard() {
    const o = state.overview;
    if (!o) {
      inspector.innerHTML = `<div class="pane">${'<div class="skeleton"></div>'.repeat(8)}</div>`;
      return;
    }
    const st = o.status.stats;
    const conf = { exact: st.exact, inferred: st.inferred, unresolved: st.unresolved };
    const total = Math.max(1, st.edges);
    const confColors = { exact: cssVar("--exact"), inferred: cssVar("--inferred"), unresolved: cssVar("--unresolved") };
    const stack = CONFIDENCES.map((c) => `<span style="width:${(conf[c] / total) * 100}%;background:${confColors[c]}"></span>`).join("");
    const confLegend = CONFIDENCES.map(
      (c) => `<div class="li"><span class="line-swatch ${c}"></span><div><div>${esc(t("conf." + c))}</div><div class="d">${esc(t(`conf.${c}.desc`))}</div></div><span class="v">${num(conf[c])} · ${Math.round((conf[c] / total) * 100)}%</span></div>`
    ).join("");
    const relMax = Math.max(1, ...o.relations.map((r) => r[1]));
    const rels = o.relations
      .map(([r, c]) => `<div class="bar-row"><span class="bl">${esc(t("rel." + r))}</span><span class="bt"><span style="width:${(c / relMax) * 100}%;background:${REL_COLORS[r] || "#94a3b8"}"></span></span><span class="bv">${num(c)}</span></div>`)
      .join("");
    const langs = st.languages.filter(([l]) => l && l !== "config");
    const langMax = Math.max(1, ...langs.map((l) => l[1]));
    const langColors = { csharp: "#8b5cf6", typescript: "#3b82f6", javascript: "#eab308", python: "#10b981", prisma: "#14b8a6" };
    const langRows = langs
      .map(([l, c]) => `<div class="bar-row"><span class="bl">${esc(l)}</span><span class="bt"><span style="width:${(c / langMax) * 100}%;background:${langColors[l] || "#94a3b8"}"></span></span><span class="bv">${num(c)}</span></div>`)
      .join("");
    const hot = o.hotspots.slice(0, 5).map((h) => rowFor(h, `cc ${h.complexity}`)).join("");
    const entries = o.entrypoints.slice(0, 6).map((e) => rowFor(e, esc((e.entry_kind || "").replace(/^route /, "")))).join("");
    const kpi = (ic, label, v) => `<div class="kpi"><div class="kpi-label">${icon(ic, 14)}${esc(label)}</div><div class="kpi-value">${num(v)}</div></div>`;
    inspector.innerHTML = `
      <div class="insp-head"><h2 class="insp-title">${esc(t("dash.title"))}</h2><div class="insp-sub">${esc(o.status.root)}</div></div>
      <div class="pane">
        <div class="kpis">
          ${kpi("file", t("dash.files"), st.files)}${kpi("code", t("dash.symbols"), st.nodes)}
          ${kpi("link", t("dash.relations"), st.edges)}${kpi("entry", t("dash.entrypoints"), o.entrypoints.length)}
        </div>
        <div class="section"><div class="section-title">${esc(t("dash.confidence"))}</div><div class="stack">${stack}</div><div class="stack-legend">${confLegend}</div></div>
        <div class="section"><div class="section-title">${esc(t("dash.relationsBreakdown"))}</div><div class="bars">${rels}</div></div>
        ${langRows ? `<div class="section"><div class="section-title">${esc(t("dash.languages"))}</div><div class="bars">${langRows}</div></div>` : ""}
        ${entries ? `<div class="section"><div class="section-title">${esc(t("dash.entries"))}<button class="link-btn" data-view="entrypoints">${esc(t("dash.viewAll"))}</button></div><div class="rows">${entries}</div></div>` : ""}
        ${hot ? `<div class="section"><div class="section-title">${esc(t("dash.hotspots"))}<button class="link-btn" data-view="hotspots">${esc(t("dash.viewAll"))}</button></div><div class="rows">${hot}</div></div>` : ""}
      </div>`;
  }

  function listItems() {
    const o = state.overview;
    if (!o) return [];
    switch (state.view) {
      case "entrypoints":
        return o.entrypoints.map((e) => ({ html: rowFor(e, esc(e.entry_kind || "")), text: `${e.label} ${e.entry_kind} ${e.coords}` }));
      case "models":
        return o.models.map((m) => ({ html: rowFor(m, `${esc(m.framework || "")} · ${m.fields}`), text: `${m.label} ${m.framework} ${m.coords}` }));
      case "hotspots":
        return o.hotspots.map((h) => ({ html: rowFor(h, `cc ${h.complexity}`), text: `${h.label} ${h.coords}` }));
      case "unresolved":
        return o.unresolved.map((u) => ({
          text: `${u.source_label} ${u.unresolved_symbol} ${u.evidence}`,
          html: `<div class="row clickable" data-open="${esc(u.source)}" data-label="${esc(u.source_label || "")}">
            <span class="kind-dot" style="background:${kindColor("unresolved")}"></span>
            <div class="main"><div class="title">${esc(u.source_label || "")} → ${esc(u.unresolved_symbol || "")}</div><div class="sub">${esc(u.evidence)}</div><div class="sub">${esc(u.site)}</div></div>
            <span class="rel-tag">${esc(t("rel." + u.relation))}</span></div>`,
        }));
      default:
        return [];
    }
  }

  function renderList() {
    const all = listItems();
    const f = state.listFilter.toLowerCase();
    const items = f ? all.filter((i) => i.text.toLowerCase().includes(f)) : all;
    inspector.innerHTML = `
      <div class="insp-head"><h2 class="insp-title">${esc(t("view." + state.view))}</h2><div class="insp-sub">${esc(t("list.count", { n: num(items.length) }))}</div></div>
      <div class="pane">
        <div class="list-head"><input class="text-input" id="list-filter" placeholder="${esc(t("list.filter"))}" value="${esc(state.listFilter)}" /></div>
        ${items.length ? `<div class="rows">${items.map((i) => i.html).join("")}</div>` : `<div class="empty-note">${esc(t("list.empty"))}</div>`}
      </div>`;
    const input = $("#list-filter");
    input.addEventListener("input", () => {
      state.listFilter = input.value;
      const pos = input.selectionStart;
      renderList();
      const again = $("#list-filter");
      again.focus();
      again.setSelectionRange(pos, pos);
    });
  }

  function editorUrl(n) {
    const root = (state.status?.root || "").replace(/\\/g, "/");
    return `vscode://file/${root}/${n.file}:${n.start}`;
  }

  function nodeHeader(n, d) {
    const badges = [kindBadge(n.kind)];
    if (n.entry_kind) badges.push(`<span class="badge entry">${icon("entry", 12)}${esc(n.entry_kind)}</span>`);
    if (n.is_test) badges.push(`<span class="badge test">${icon("tests", 12)}${esc(t("ov.test"))}</span>`);
    const evCount = d.outgoing.length + d.incoming.length;
    const tabs = TABS.map(
      (tb) => `<button class="tab ${state.tab === tb.id ? "active" : ""}" data-tab="${tb.id}" title="${esc(t("tabFull." + tb.id))}">${esc(t("tab." + tb.id))}${tb.id === "evidence" ? `<span class="count">${evCount}</span>` : ""}</button>`
    ).join("");
    return `
      <div class="insp-head">
        <div class="badges">${badges.join("")}</div>
        <h2 class="insp-title">${esc(n.label)}</h2>
        ${n.qualified_name && n.qualified_name !== n.label ? `<div class="insp-sub">${esc(n.qualified_name)}</div>` : ""}
        <div class="coord-row">
          <div class="coord"><span>${esc(n.coords)}</span><button class="icon-btn sm" data-act="coords" title="${esc(t("insp.copyCoords"))}">${icon("copy", 14)}</button></div>
        </div>
        <div class="action-row">
          <button class="btn primary sm" data-act="coords">${icon("copy", 13)}${esc(t("insp.copyCoords"))}</button>
          <button class="btn sm" data-act="focus">${icon("focus", 13)}${esc(t("insp.focus"))}</button>
          <a class="btn sm" href="${esc(editorUrl(n))}" style="text-decoration:none">${icon("external", 13)}${esc(t("insp.openEditor"))}</a>
        </div>
      </div>
      <nav class="tabs">${tabs}</nav>`;
  }

  function complexityMeter(cc) {
    if (!cc) return "—";
    const level = cc >= 10 ? "high" : cc >= 5 ? "mid" : "low";
    const color = level === "high" ? cssVar("--bad") : level === "mid" ? cssVar("--warn") : cssVar("--ok");
    return `<span class="meter"><b>${cc}</b><span class="meter-bar"><span style="width:${Math.min(100, cc * 7)}%;background:${color}"></span></span><span class="muted">${esc(t("ov.cc." + level))}</span></span>`;
  }

  function paneOverview(n, d) {
    const p = n.params;
    const params = p ? t("ov.paramsValue", { t: p.total, r: p.required }) + (p.variadic ? `, ${t("ov.variadic")}` : "") : "—";
    const chips = (arr, cls = "") => arr.map((a) => `<span class="chip ${cls}" title="${esc(a)}">${esc(a)}</span>`).join("");
    const members = (d.children || [])
      .slice(0, 80)
      .map((c) => `<div class="row clickable" data-open="${esc(c.id)}" data-label="${esc(c.label)}"><span class="kind-dot" style="background:${kindColor(c.kind)}"></span><div class="main"><div class="title">${esc(c.signature || c.label)}</div><div class="sub">${esc(c.coords)}</div></div></div>`)
      .join("");
    const muts = (d.mutations || []).map((m) => `<tr><td><span class="chip">${esc(m.type)}</span></td><td><code>${esc(m.target)}</code></td><td class="muted">:${m.line}</td></tr>`).join("");
    return `
      ${n.signature ? `<div class="section"><div class="section-title">${esc(t("ov.signature"))}</div><pre class="code-block">${esc(n.signature)}</pre>${n.docstring ? `<p class="doc">${esc(n.docstring)}</p>` : ""}</div>` : ""}
      <div class="section"><div class="props">
        <div class="k">${esc(t("ov.lines"))}</div><div class="mono">${n.start}–${n.end} <span class="muted">(${esc(t("ov.lineCount", { n: n.end - n.start + 1 }))})</span></div>
        <div class="k">${esc(t("ov.file"))}</div><div class="mono">${esc(n.file)}</div>
        <div class="k">${esc(t("ov.language"))}</div><div>${esc(n.language)}</div>
        <div class="k">${esc(t("ov.complexity"))}</div><div>${complexityMeter(n.complexity)}</div>
        <div class="k">${esc(t("ov.visibility"))}</div><div>${esc(n.visibility || "—")}</div>
        <div class="k">${esc(t("ov.params"))}</div><div>${esc(params)}</div>
        ${n.type_ref ? `<div class="k">${esc(t("ov.type"))}</div><div class="mono">${esc(n.type_ref)}</div>` : ""}
        ${d.parent ? `<div class="k">${esc(t("ov.declaredIn"))}</div><div><button class="link-btn" data-open="${esc(d.parent.id)}" data-label="${esc(d.parent.label)}">${esc(d.parent.label)}</button></div>` : ""}
      </div></div>
      ${n.capabilities?.length ? `<div class="section"><div class="section-title">${esc(t("ov.capabilities"))}</div><div class="chips">${chips(n.capabilities, "cap")}</div></div>` : ""}
      ${n.attrs?.length ? `<div class="section"><div class="section-title">${esc(t("ov.attributes"))}</div><div class="chips">${chips(n.attrs.map((a) => "@" + a))}</div></div>` : ""}
      ${n.bases?.length ? `<div class="section"><div class="section-title">${esc(t("ov.bases"))}</div><div class="chips">${chips(n.bases)}</div></div>` : ""}
      ${muts ? `<div class="section"><div class="section-title">${esc(t("ov.mutations"))}</div><table class="grid"><tbody>${muts}</tbody></table></div>` : ""}
      ${members ? `<div class="section"><div class="section-title">${esc(t("ov.members"))}<span class="count">${d.children.length}</span></div><div class="rows">${members}</div></div>` : ""}`;
  }

  function evCard(e, dir) {
    const o = e.other;
    const target = o
      ? `<span class="ev-target link" data-open="${esc(o.id)}" data-label="${esc(o.label)}">${esc(o.label)}</span>`
      : `<span class="ev-target muted">? ${esc(e.unresolved_symbol || "")}</span>`;
    return `<div class="ev ${e.confidence}">
      <div class="ev-top"><span class="dir-icon">${icon(dir === "out" ? "out" : "in", 14)}</span>${target}<span class="conf-badge ${e.confidence}">${esc(t("conf." + e.confidence))}</span></div>
      <div class="ev-text">${esc(e.evidence)}</div>
      <div class="ev-site">${esc(e.site)}${o ? " → " + esc(o.coords) : ""} · ${esc(e.resolver)}</div></div>`;
  }

  function paneEvidence(d) {
    const f = state.evFilter;
    const keep = (e) => f === "all" || e.confidence === f;
    const seg = ["all", ...CONFIDENCES]
      .map((c) => `<button class="${f === c ? "on" : ""}" data-evf="${c}">${esc(c === "all" ? t("ev.all") : t("conf." + c))}</button>`)
      .join("");
    const group = (edges, dir) => {
      const by = {};
      edges.filter(keep).forEach((e) => (by[e.relation] ||= []).push(e));
      return Object.entries(by)
        .map(([rel, list]) => `<div class="ev-group"><div class="ev-group-head"><span class="swatch" style="background:${REL_COLORS[rel] || "#94a3b8"}"></span>${esc(t("rel." + rel))}<span class="count">${list.length}</span></div>${list.map((e) => evCard(e, dir)).join("")}</div>`)
        .join("");
    };
    const outs = group(d.outgoing, "out");
    const ins = group(d.incoming, "in");
    return `
      <div class="list-head"><div class="segmented">${seg}</div></div>
      <div class="section"><div class="section-title">${esc(t("ev.outgoing"))}</div>${outs || `<div class="empty-note">${esc(t("ev.none"))}</div>`}</div>
      <div class="section"><div class="section-title">${esc(t("ev.incoming"))}</div>${ins || `<div class="empty-note">${esc(t("ev.none"))}</div>`}</div>`;
  }

  const skeleton = (n = 6) => `<div class="muted" style="margin-bottom:10px">${esc(t("common.loading"))}</div>${'<div class="skeleton"></div>'.repeat(n)}`;

  async function paneFlows(n, d) {
    const key = `flows:${n.id}`;
    if (state.cache[key]) return state.cache[key];
    const hasHttp = (d.routes || []).length || d.outgoing.some((e) => e.relation === "HTTP_CALLS") || d.incoming.some((e) => e.relation === "HTTP_CALLS");
    const jobs = [];
    if (hasHttp) jobs.push(tool("trace_fullstack_flow", { api_route_or_call: n.id, max_token_budget: 1800 }).then((r) => ["flows.fullstack", {}, r.text]));
    jobs.push(tool("get_call_hierarchy", { symbol_id: n.id, direction: "both", depth: 2, max_token_budget: 1800 }).then((r) => ["flows.calls", {}, r.text]));
    const thrown = [...new Set(d.outgoing.filter((e) => e.relation === "THROWS").map((e) => (e.other ? e.other.label : e.unresolved_symbol)))].slice(0, 2);
    for (const x of thrown) jobs.push(tool("trace_error_flow", { exception_name_or_message: x, max_token_budget: 900 }).then((r) => ["flows.errors", { e: x }, r.text]));
    const parts = (await Promise.all(jobs)).map(([k, v, text]) => `<div class="section"><div class="section-title">${esc(t(k, v))}</div><pre class="tool">${esc(text)}</pre></div>`);
    return (state.cache[key] = parts.join(""));
  }

  async function paneData(n, d) {
    if (d.data_model) {
      const rows = d.data_model.fields
        .map((f) => `<tr><td>${f.pk ? '<span class="key pk">PK</span>' : ""}${f.fk ? '<span class="key fk">FK</span>' : ""}${esc(f.name)}</td><td><code>${esc(f.type || "")}</code></td><td>${f.target ? `<button class="link-btn" data-search="${esc(f.target)}">${esc(f.target)}</button>` : ""}</td><td>${esc(f.cardinality || "")}</td></tr>`)
        .join("");
      return `<div class="section"><div class="section-title">${esc(t("data.entity", { fw: d.data_model.framework || "" }))}</div>
        <table class="grid"><thead><tr><th>${esc(t("data.field"))}</th><th>${esc(t("data.type"))}</th><th>${esc(t("data.ref"))}</th><th>${esc(t("data.card"))}</th></tr></thead><tbody>${rows}</tbody></table></div>`;
    }
    if (["class", "interface", "struct", "enum", "type_alias"].includes(n.kind)) {
      const key = `contract:${n.id}`;
      state.cache[key] ||= (await tool("get_type_contract", { type_id: n.id })).text;
      return `<div class="section"><div class="section-title">${esc(t("data.contract"))}</div><pre class="tool">${esc(state.cache[key])}</pre></div>`;
    }
    return `<div class="empty-note">${esc(t("data.none"))}</div>`;
  }

  // Lightweight syntax highlighting (keywords, strings, comments, numbers, types).
  const KEYWORDS = new Set(
    ("abstract as async await base bool break case catch class const constructor continue def default del delegate do elif else enum " +
      "except export extends false finally for foreach from function get global if implements import in instanceof interface internal is " +
      "lambda let namespace new none not null object of or override pass private protected public raise readonly record return sealed self " +
      "set static string struct super switch this throw true try type typeof using var virtual void while with yield int long double float " +
      "decimal char True False None and nonlocal assert model").split(" ")
  );
  function syntax(line, lang) {
    const commentRe = lang === "python" ? "#.*$" : "\\/\\/.*$|\\/\\*.*?\\*\\/";
    const re = new RegExp(`(${commentRe})|("(?:[^"\\\\]|\\\\.)*"|'(?:[^'\\\\]|\\\\.)*'|\`(?:[^\`\\\\]|\\\\.)*\`)|(\\b\\d[\\d_.]*\\b)|([A-Za-z_][A-Za-z0-9_]*)`, "g");
    let out = "";
    let last = 0;
    for (const m of line.matchAll(re)) {
      out += esc(line.slice(last, m.index));
      const [tok, c, s, n, w] = m;
      if (c) out += `<span class="tok-c">${esc(c)}</span>`;
      else if (s) out += `<span class="tok-s">${esc(s)}</span>`;
      else if (n) out += `<span class="tok-n">${esc(n)}</span>`;
      else if (w && KEYWORDS.has(w)) out += `<span class="tok-k">${esc(w)}</span>`;
      else if (w && /^[A-Z][a-z0-9]/.test(w)) out += `<span class="tok-t">${esc(w)}</span>`;
      else out += esc(tok);
      last = m.index + tok.length;
    }
    return out + esc(line.slice(last));
  }

  async function paneSource(n) {
    const key = `src:${n.id}`;
    state.cache[key] ||= await api(`/api/source/${encodeURIComponent(n.id)}`);
    const s = state.cache[key];
    // Remove the indentation shared by every non-empty line (the symbol's nesting level).
    const indents = s.lines.filter((l) => l.text.trim()).map((l) => l.text.match(/^[ 	]*/)[0].replace(/	/g, "    ").length);
    const cut = indents.length ? Math.min(...indents) : 0;
    const strip = (text) => {
      let i = 0;
      let w = 0;
      while (i < text.length && w < cut && (text[i] === " " || text[i] === "	")) {
        w += text[i] === "	" ? 4 : 1;
        i++;
      }
      return text.slice(i);
    };
    const lines = s.lines.map((l) => `<div class="cl"><span class="ln">${l.n}</span><span class="lc">${syntax(strip(l.text), s.language) || " "}</span></div>`).join("");
    return `<div class="codeview">
      <div class="codeview-head"><span>${esc(s.file)}:${s.start}-${s.end}</span><button class="btn sm" data-act="copy-source">${icon("copy", 13)}${esc(t("src.copy"))}</button></div>
      <div class="codeview-body">${lines}</div></div>
      ${s.truncated ? `<p class="muted">${esc(t("src.truncated", { n: s.lines.length }))}</p>` : ""}`;
  }

  function paneActions(n) {
    const a = (act, ic, title, sub) => `<button class="action" data-act="${act}"><span class="a-icon">${icon(ic, 15)}</span><span class="a-main"><div class="a-title">${esc(title)}</div>${sub ? `<div class="a-sub">${esc(sub)}</div>` : ""}</span></button>`;
    return `<div class="actions-grid">
      ${a("coords", "copy", t("act.coords"), n.coords)}
      ${a("id", "hash", t("act.id"), n.id)}
      ${a("body", "code", t("act.body"))}
      ${a("skeleton", "file", t("act.skeleton"), n.file)}
      ${a("impact", "target", t("act.impact"))}
      ${a("tests", "tests", t("act.tests"))}
      ${a("expand", "expand", t("act.expand"))}
      ${a("editor", "external", t("act.editor"))}
    </div><div class="section" style="margin-top:14px" id="action-out"></div>`;
  }

  async function renderNode() {
    const d = state.detail;
    const n = d.node;
    inspector.innerHTML = nodeHeader(n, d) + `<div class="pane" id="pane"></div>`;
    const pane = $("#pane");
    const id = n.id;
    const tab = state.tab;
    const sync = { overview: () => paneOverview(n, d), evidence: () => paneEvidence(d), actions: () => paneActions(n) };
    if (sync[tab]) {
      pane.innerHTML = sync[tab]();
      return;
    }
    pane.innerHTML = skeleton();
    try {
      const html = tab === "flows" ? await paneFlows(n, d) : tab === "data" ? await paneData(n, d) : await paneSource(n);
      if (state.selected === id && state.tab === tab && $("#pane")) $("#pane").innerHTML = html;
    } catch (e) {
      pane.innerHTML = `<div class="empty-note">${esc(t("common.error"))}: ${esc(e.message)}</div>`;
    }
  }

  async function select(id) {
    try {
      const d = await api(`/api/node/${encodeURIComponent(id)}`);
      state.selected = id;
      state.detail = d;
      state.mode = "node";
      inspector.scrollTop = 0;
      renderNode();
      const node = cy.getElementById(id);
      if (node.length) {
        cy.elements().unselect();
        node.select();
      }
    } catch (e) {
      toast(e.message, "error");
    }
  }

  async function runAction(act) {
    const d = state.detail;
    if (!d) return;
    const n = d.node;
    const show = (title, text) => {
      const out = $("#action-out");
      if (out) out.innerHTML = `<div class="section-title">${esc(title)}</div><pre class="tool">${esc(text)}</pre>`;
    };
    switch (act) {
      case "coords":
        return copy(n.coords, t("insp.coordsCopied", { c: n.coords }));
      case "id":
        return copy(n.id);
      case "focus":
        return focusNode(n.id, n.label);
      case "expand":
        state.depth = 2;
        return focusNode(n.id, n.label);
      case "editor":
        location.href = editorUrl(n);
        return;
      case "copy-source": {
        const s = state.cache[`src:${n.id}`];
        if (s) copy(s.lines.map((l) => `${String(l.n).padStart(4)}| ${l.text}`).join("\n"));
        return;
      }
      case "body": {
        const r = await tool("get_symbol_body", { symbol_id: n.id });
        show(t("act.body"), r.text);
        return copy(r.text);
      }
      case "skeleton": {
        const r = await tool("get_file_skeleton", { file_path: n.file });
        show(t("act.skeleton"), r.text);
        return copy(r.text);
      }
      case "impact": {
        const r = await tool("get_impact_radius", { symbol_id: n.id, depth: 3, max_token_budget: 1500 });
        return show(t("act.impact"), r.text);
      }
      case "tests": {
        const r = await tool("get_tests_for_symbol", { symbol_id: n.id });
        return show(t("act.tests"), r.text);
      }
    }
  }

  function setView(id) {
    state.view = id;
    state.listFilter = "";
    $("#sidebar").classList.remove("open");
    if (id === "graph") {
      goHome();
      return;
    }
    state.mode = id === "dashboard" ? "dashboard" : "list";
    inspector.scrollTop = 0;
    renderInspector();
    renderSidebar();
  }

  // ------------------------------------------------------------------ palette
  const palette = $("#palette");
  const pInput = $("#palette-input");
  const pResults = $("#palette-results");
  let pItems = [];
  let pSel = 0;
  let pSeq = 0;

  function commands() {
    return [
      { title: t("cmd.overview"), icon: "dashboard", run: () => setView("dashboard") },
      { title: t("cmd.graph"), icon: "graph", run: () => setView("graph") },
      { title: t("cmd.fit"), icon: "fit", run: () => cy.animate({ fit: { padding: 40 } }, { duration: 250 }) },
      { title: t("cmd.reindex"), icon: "refresh", run: reindex },
      { title: t("cmd.theme"), icon: effectiveTheme() === "dark" ? "sun" : "moon", run: toggleTheme },
      { title: t("cmd.export"), icon: "download", run: exportPng },
      ...VIEWS.filter((v) => !["dashboard", "graph"].includes(v.id)).map((v) => ({ title: t("cmd.view", { v: t("view." + v.id) }), icon: v.icon, run: () => setView(v.id) })),
      ...Object.entries(I18N.langs).map(([code, l]) => ({ title: t("cmd.lang", { l: l.name }), icon: "globe", run: () => setLanguage(code) })),
    ];
  }

  function markMatch(text, q) {
    if (!q) return esc(text);
    const i = text.toLowerCase().indexOf(q.toLowerCase());
    if (i < 0) return esc(text);
    return `${esc(text.slice(0, i))}<mark>${esc(text.slice(i, i + q.length))}</mark>${esc(text.slice(i + q.length))}`;
  }

  function renderPalette(q, symbols, pending = false) {
    const cmdHits = commands().filter((c) => !q || c.title.toLowerCase().includes(q.toLowerCase())).slice(0, q ? 6 : 9);
    pItems = [];
    let html = "";
    if (symbols.length) {
      html += `<div class="p-group">${esc(t("palette.symbols"))}</div>`;
      for (const s of symbols) {
        const i = pItems.push({ run: () => focusNode(s.id, s.label) }) - 1;
        html += `<div class="p-item" data-i="${i}"><span class="p-icon"><span class="swatch" style="background:${kindColor(s.kind)}"></span></span>
          <div class="p-main"><div class="p-title">${markMatch(s.label, q)}</div><div class="p-sub">${esc(s.coords)}</div></div><span class="p-kind">${esc(t("kind." + s.kind))}</span></div>`;
      }
    }
    if (cmdHits.length) {
      html += `<div class="p-group">${esc(t("palette.commands"))}</div>`;
      for (const c of cmdHits) {
        const i = pItems.push({ run: c.run }) - 1;
        html += `<div class="p-item" data-i="${i}"><span class="p-icon">${icon(c.icon, 14)}</span><div class="p-main"><div class="p-title">${markMatch(c.title, q)}</div></div></div>`;
      }
    }
    if (pending) html += `<div class="p-empty">${esc(t("common.loading"))}</div>`;
    else if (!pItems.length) html = `<div class="p-empty">${esc(t("palette.noResults", { q }))}</div>`;
    pResults.innerHTML = html;
    pSel = 0;
    paintSel();
  }

  function paintSel() {
    $$(".p-item", pResults).forEach((el) => el.classList.toggle("sel", Number(el.dataset.i) === pSel));
    $(`.p-item[data-i="${pSel}"]`, pResults)?.scrollIntoView({ block: "nearest" });
  }

  // Commands filter instantly; symbol results arrive after a short debounce. Enter waits for them.
  let pTimer = null;
  let pPending = null;
  function searchPalette() {
    const q = pInput.value.trim();
    const seq = ++pSeq;
    clearTimeout(pTimer);
    if (q.length < 2) {
      pPending = null;
      return renderPalette(q, []);
    }
    renderPalette(q, [], true);
    pPending = new Promise((resolve) => {
      pTimer = setTimeout(async () => {
        try {
          const r = await api(`/api/search?q=${encodeURIComponent(q)}&limit=12`);
          if (seq === pSeq) renderPalette(q, r.results);
        } catch {
          if (seq === pSeq) renderPalette(q, []);
        }
        if (seq === pSeq) pPending = null;
        resolve();
      }, 90);
    });
  }

  function openPalette() {
    palette.hidden = false;
    pInput.value = "";
    renderPalette("", []);
    pInput.focus();
  }
  function closePalette() {
    palette.hidden = true;
  }
  function runPalette(i) {
    const it = pItems[i];
    closePalette();
    it?.run();
  }
  pInput.addEventListener("input", searchPalette);
  pInput.addEventListener("keydown", (ev) => {
    if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      if (!pItems.length) return;
      pSel = (pSel + (ev.key === "ArrowDown" ? 1 : -1) + pItems.length) % pItems.length;
      paintSel();
    } else if (ev.key === "Enter") {
      ev.preventDefault();
      if (pPending) pPending.then(() => runPalette(pSel));
      else runPalette(pSel);
    } else if (ev.key === "Escape") {
      closePalette();
    }
  });
  pResults.addEventListener("click", (ev) => {
    const it = ev.target.closest(".p-item");
    if (it) runPalette(Number(it.dataset.i));
  });
  pResults.addEventListener("mousemove", (ev) => {
    const it = ev.target.closest(".p-item");
    if (it && Number(it.dataset.i) !== pSel) {
      pSel = Number(it.dataset.i);
      paintSel();
    }
  });
  palette.addEventListener("mousedown", (ev) => {
    if (ev.target === palette) closePalette();
  });

  // ------------------------------------------------------------------ status, overview, reindex
  async function loadOverview() {
    try {
      state.overview = await api("/api/overview");
      state.status = state.overview.status;
      renderSidebar();
      renderStatus();
      if (state.mode !== "node") renderInspector();
    } catch (e) {
      toast(e.message, "error");
    }
  }

  async function pollStatus() {
    try {
      state.status = await api("/api/status");
      renderStatus();
      const li = state.status.stats.last_indexed_at;
      if (state.lastIndexed !== null && li !== state.lastIndexed) {
        state.lastIndexed = li;
        state.cache = {};
        await loadOverview();
        loadGraph();
        if (state.selected && state.mode === "node") select(state.selected);
      } else {
        state.lastIndexed = li;
        renderSidebar();
      }
    } catch {
      state.status = null;
      renderStatus();
    }
  }

  async function reindex() {
    const btn = $("#btn-reindex");
    btn.classList.add("spin");
    toast(t("toast.reindexing"));
    try {
      const r = await api("/api/reindex", { method: "POST" });
      toast(t("toast.reindexed", { n: r.files_parsed, ms: r.duration_ms }));
      await pollStatus();
    } catch (e) {
      toast(e.message, "error");
    } finally {
      btn.classList.remove("spin");
    }
  }

  // ------------------------------------------------------------------ events
  document.addEventListener("click", (ev) => {
    const el = ev.target;
    if (el.closest("#palette")) return;
    const view = el.closest("[data-view]");
    if (view) return setView(view.dataset.view);
    const open = el.closest("[data-open]");
    if (open) {
      const id = open.dataset.open;
      const node = cy.getElementById(id);
      if (node.length && state.focus) {
        highlightNeighbourhood(node);
        select(id);
      } else focusNode(id, open.dataset.label || id);
      return;
    }
    const search = el.closest("[data-search]");
    if (search) {
      openPalette();
      pInput.value = search.dataset.search;
      searchPalette();
      return;
    }
    const tab = el.closest("[data-tab]");
    if (tab) {
      state.tab = tab.dataset.tab;
      store.set("tab", state.tab);
      return renderNode();
    }
    const evf = el.closest("[data-evf]");
    if (evf) {
      state.evFilter = evf.dataset.evf;
      return renderNode();
    }
    const act = el.closest("[data-act]");
    if (act) return runAction(act.dataset.act);
    const rel = el.closest("[data-rel]");
    if (rel) {
      const r = rel.dataset.rel;
      state.relations.has(r) ? state.relations.delete(r) : state.relations.add(r);
      store.set("relations", [...state.relations]);
      renderSidebar();
      return loadGraph();
    }
    const conf = el.closest("[data-conf]");
    if (conf) {
      const c = conf.dataset.conf;
      state.conf.has(c) ? state.conf.delete(c) : state.conf.add(c);
      store.set("confidence", [...state.conf]);
      renderSidebar();
      return applyConfidenceFilter();
    }
    const crumb = el.closest("[data-crumb]");
    if (crumb) {
      const id = crumb.dataset.crumb;
      if (id === "home") return goHome();
      const idx = state.history.findIndex((h) => h.id === id);
      state.history = state.history.slice(0, idx + 1);
      return focusNode(id, null, { push: false });
    }
    const depth = el.closest("[data-depth]");
    if (depth) {
      state.depth = Number(depth.dataset.depth);
      renderStageBar();
      return loadGraph();
    }
  });

  $("#open-palette").addEventListener("click", openPalette);
  $("#btn-theme").addEventListener("click", toggleTheme);
  $("#btn-reindex").addEventListener("click", reindex);
  $("#lang-select").addEventListener("change", (ev) => setLanguage(ev.target.value));
  $("#layout-select").addEventListener("change", (ev) => {
    state.layout = ev.target.value;
    store.set("layout", state.layout);
    runLayout(state.focus);
  });
  $("#btn-back").addEventListener("click", () => {
    state.history.pop();
    const prev = state.history[state.history.length - 1];
    if (prev) focusNode(prev.id, prev.label, { push: false });
    else goHome();
  });
  $("#btn-sidebar").addEventListener("click", () => $("#sidebar").classList.toggle("open"));
  const zoomBy = (f) => cy.zoom({ level: cy.zoom() * f, renderedPosition: { x: cy.width() / 2, y: cy.height() / 2 } });
  $("#ctl-zoom-in").addEventListener("click", () => zoomBy(1.25));
  $("#ctl-zoom-out").addEventListener("click", () => zoomBy(0.8));
  $("#ctl-fit").addEventListener("click", () => cy.animate({ fit: { padding: 40 } }, { duration: 250 }));
  $("#ctl-export").addEventListener("click", exportPng);
  $("#legend-toggle").addEventListener("click", () => {
    const collapsed = !$("#legend").classList.contains("collapsed");
    $("#legend").classList.toggle("collapsed", collapsed);
    store.set("legendCollapsed", collapsed);
  });

  document.addEventListener("keydown", (ev) => {
    const typing = ["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement?.tagName);
    if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "k") {
      ev.preventDefault();
      palette.hidden ? openPalette() : closePalette();
    } else if (ev.key === "/" && !typing && palette.hidden) {
      ev.preventDefault();
      openPalette();
    } else if (ev.key === "Escape" && !palette.hidden) {
      closePalette();
    } else if (!typing && palette.hidden && !ev.ctrlKey && !ev.metaKey) {
      if (ev.key === "f") cy.animate({ fit: { padding: 40 } }, { duration: 250 });
      if (ev.key === "Backspace" && state.history.length) $("#btn-back").click();
    }
  });

  // Resizable inspector.
  const resizer = $("#resizer");
  const app = $("#app");
  const savedWidth = store.get("inspectorWidth", null);
  if (savedWidth) app.style.setProperty("--inspector-w", `${savedWidth}px`);
  resizer.addEventListener("mousedown", (ev) => {
    ev.preventDefault();
    resizer.classList.add("dragging");
    const move = (e) => {
      const w = Math.min(Math.max(window.innerWidth - e.clientX, 320), Math.max(320, Math.min(900, window.innerWidth - 480)));
      app.style.setProperty("--inspector-w", `${w}px`);
    };
    const up = () => {
      resizer.classList.remove("dragging");
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", up);
      store.set("inspectorWidth", parseInt(app.style.getPropertyValue("--inspector-w"), 10) || null);
    };
    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", up);
  });
  new ResizeObserver(() => cy.resize()).observe($("#cy"));

  // ------------------------------------------------------------------ boot
  hydrateIcons();
  renderThemeIcon();
  renderLangSelect();
  renderChrome();
  renderInspector();
  loadOverview().then(() => (state.lastIndexed = state.status?.stats?.last_indexed_at ?? null));
  loadGraph();
  setInterval(pollStatus, 1500);
  setInterval(() => renderSidebar(), 30000);
})();
