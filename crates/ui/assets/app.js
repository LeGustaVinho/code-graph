/* CodeGraph Explorer — embedded SPA (Cytoscape.js). */
(() => {
  "use strict";

  const $ = (s) => document.querySelector(s);
  const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const css = (v) => getComputedStyle(document.documentElement).getPropertyValue(v).trim();

  const ALL_RELATIONS = ["CALLS", "HTTP_CALLS", "RESOLVES_DI", "INHERITS", "THROWS", "CAUGHT_BY", "TESTS", "IMPORTS", "READS_VAR", "WRITES_VAR", "USES_LITERAL"];
  const DEFAULT_ON = new Set(["CALLS", "HTTP_CALLS", "RESOLVES_DI", "INHERITS", "THROWS", "CAUGHT_BY", "TESTS"]);
  const KIND_COLORS = {
    class: "#6c8cff", interface: "#a78bfa", struct: "#6c8cff", enum: "#f472b6", type_alias: "#a78bfa",
    method: "#34c38f", constructor: "#22a06b", function: "#2bb3a3", test: "#f5a524", data_model: "#ef8f3c",
    module: "#8b93a7", field: "#94a3b8", property: "#94a3b8", variable: "#94a3b8", const: "#c084fc",
    literal: "#d6b656", unresolved: "#e5484d",
  };
  const KIND_SHAPES = {
    class: "round-rectangle", interface: "diamond", struct: "round-rectangle", enum: "hexagon", data_model: "barrel",
    module: "rectangle", test: "tag", literal: "tag", unresolved: "octagon", const: "pentagon",
  };

  const state = {
    relations: new Set(DEFAULT_ON),
    focus: null,
    selected: null,
    lastIndexed: null,
    detail: null,
  };

  // ---------------------------------------------------------------- API
  async function api(path, opts) {
    const res = await fetch(path, opts);
    const body = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(body.error || res.statusText);
    return body;
  }
  const tool = (name, args) => api(`/api/tool/${name}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(args) });

  function toast(msg) {
    const t = $("#toast");
    t.textContent = msg;
    t.hidden = false;
    clearTimeout(toast._t);
    toast._t = setTimeout(() => (t.hidden = true), 1800);
  }

  async function copy(text, label) {
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
    toast(label || "Copiado");
  }

  // ---------------------------------------------------------------- Graph
  const kindStyles = Object.keys(KIND_COLORS).map((k) => ({
    selector: `node[kind = '${k}']`,
    style: { "background-color": KIND_COLORS[k], shape: KIND_SHAPES[k] || "ellipse" },
  }));

  const cy = cytoscape({
    container: $("#cy"),
    minZoom: 0.08,
    maxZoom: 3,
    style: [
      {
        selector: "node",
        style: {
          "background-color": "#8b93a7",
          shape: "ellipse",
          label: "data(label)",
          "font-size": 10,
          "font-family": css("--sans") || "sans-serif",
          color: css("--text"),
          "text-valign": "bottom",
          "text-margin-y": 4,
          "text-outline-width": 2,
          "text-outline-color": css("--bg"),
          width: 20,
          height: 20,
        },
      },
      ...kindStyles,
      { selector: "node[kind = 'module']", style: { width: 16, height: 16 } },
      { selector: "node[kind = 'class'], node[kind = 'data_model'], node[kind = 'interface']", style: { width: 26, height: 26 } },
      { selector: "node[?is_entrypoint]", style: { "border-width": 3, "border-color": "#f5a524" } },
      { selector: "node[kind = 'unresolved']", style: { "background-opacity": 0.15, "border-width": 2, "border-style": "dashed", "border-color": css("--unresolved"), color: css("--unresolved") } },
      { selector: "node.focus", style: { "border-width": 4, "border-color": css("--accent"), "font-weight": "bold", "font-size": 12 } },
      { selector: "node:selected", style: { "overlay-color": css("--accent"), "overlay-opacity": 0.18, "overlay-padding": 6 } },
      { selector: "node.dim", style: { opacity: 0.25 } },
      {
        selector: "edge",
        style: {
          width: 1.6,
          "curve-style": "bezier",
          "target-arrow-shape": "triangle",
          "arrow-scale": 0.8,
          "line-color": css("--exact"),
          "target-arrow-color": css("--exact"),
          opacity: 0.8,
          "font-size": 8,
          color: css("--muted"),
          "text-rotation": "autorotate",
          "text-background-color": css("--bg"),
          "text-background-opacity": 0.8,
          "text-background-padding": 1,
        },
      },
      { selector: "edge[confidence = 'inferred']", style: { "line-style": "dashed", "line-dash-pattern": [6, 4], "line-color": css("--inferred"), "target-arrow-color": css("--inferred") } },
      { selector: "edge[confidence = 'unresolved']", style: { "line-style": "dotted", "line-color": css("--unresolved"), "target-arrow-color": css("--unresolved"), "target-arrow-shape": "tee", width: 2 } },
      { selector: "edge[relation != 'CALLS']", style: { label: "data(relation)" } },
      { selector: "edge[relation = 'INHERITS']", style: { "target-arrow-shape": "triangle", "target-arrow-fill": "hollow" } },
      { selector: "edge[relation = 'HTTP_CALLS']", style: { width: 2.6 } },
      { selector: "edge.dim", style: { opacity: 0.12 } },
      { selector: "edge.hl", style: { width: 3, opacity: 1 } },
    ],
  });

  function relationsParam() {
    return [...state.relations].join(",");
  }

  function render(data, focusId) {
    const els = [];
    for (const n of data.nodes) els.push({ group: "nodes", data: n });
    const ids = new Set(data.nodes.map((n) => n.id));
    for (const e of data.edges) {
      if (ids.has(e.source) && ids.has(e.target)) els.push({ group: "edges", data: e });
    }
    cy.elements().remove();
    cy.add(els);
    $("#empty").hidden = cy.nodes().length > 0;
    const layout = focusId
      ? { name: "concentric", animate: false, minNodeSpacing: 34, concentric: (n) => (n.id() === focusId ? 100 : n.degree()), levelWidth: () => 2 }
      : { name: "cose", animate: false, nodeRepulsion: 9000, idealEdgeLength: 90, numIter: 1200, randomize: true };
    cy.layout(layout).run();
    if (focusId) {
      const f = cy.getElementById(focusId);
      f.addClass("focus");
    }
    cy.fit(undefined, 40);
    if (cy.zoom() > 1.3) {
      cy.zoom(1.3);
      cy.center();
    }
  }

  async function loadOverview() {
    state.focus = null;
    const data = await api(`/api/graph?relations=${relationsParam()}&limit=220`);
    render(data, null);
  }

  async function focus(id, depth = 1) {
    state.focus = id;
    const data = await api(`/api/graph?focus=${encodeURIComponent(id)}&depth=${depth}&relations=${relationsParam()}`);
    render(data, id);
    select(id);
  }

  cy.on("tap", "node", (ev) => {
    const id = ev.target.id();
    if (id.startsWith("unresolved:")) return;
    highlight(ev.target);
    select(id);
  });
  cy.on("dbltap", "node", (ev) => {
    const id = ev.target.id();
    if (!id.startsWith("unresolved:")) focus(id);
  });
  cy.on("tap", (ev) => {
    if (ev.target === cy) cy.elements().removeClass("dim hl");
  });

  function highlight(node) {
    cy.elements().addClass("dim").removeClass("hl");
    const nb = node.closedNeighborhood();
    nb.removeClass("dim");
    nb.edges().addClass("hl");
  }

  // ---------------------------------------------------------------- Panel
  function setTab(name) {
    document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("active", t.dataset.tab === name));
    document.querySelectorAll(".tab-page").forEach((p) => (p.hidden = p.id !== `tab-${name}`));
    if (name === "fluxos") renderFlows();
    if (name === "dados") renderData();
  }
  document.querySelectorAll(".tab").forEach((t) => t.addEventListener("click", () => setTab(t.dataset.tab)));

  function evidenceCard(e, dir) {
    const other = e.other;
    const target = other ? `<span class="t" data-id="${esc(other.id)}">${esc(other.label)}</span>` : `<span class="muted">? ${esc(e.unresolved_symbol || "")}</span>`;
    const arrow = dir === "out" ? "→" : "←";
    return `<div class="ev ${e.confidence}">
      <div class="h"><span class="rel">${esc(e.relation)}</span><span class="conf ${e.confidence}">${e.confidence}</span>${arrow} ${target}</div>
      <div class="e">${esc(e.evidence)}</div>
      <div class="s">${esc(e.site)}${other ? " · " + esc(other.coords) : ""} · ${esc(e.resolver)}</div>
    </div>`;
  }

  async function select(id) {
    let d;
    try {
      d = await api(`/api/node/${encodeURIComponent(id)}`);
    } catch (err) {
      toast(err.message);
      return;
    }
    state.selected = id;
    state.detail = d;
    const n = d.node;
    $("#panel-empty").hidden = true;
    $("#panel-body").hidden = false;
    $("#d-kind").textContent = n.kind + (n.entry_kind ? ` · ${n.entry_kind}` : "") + (n.is_test ? " · teste" : "");
    $("#d-name").textContent = n.qualified_name || n.name;
    $("#d-coords").textContent = n.coords;

    const caps = (n.capabilities || []).map((c) => `<span class="tag cap">${esc(c)}</span>`).join("");
    const attrs = (n.attrs || []).map((a) => `<span class="tag">@${esc(a)}</span>`).join("");
    const bases = (n.bases || []).map((a) => `<span class="tag">${esc(a)}</span>`).join("");
    const params = n.params ? `${n.params.total} (${n.params.required} obrigatório${n.params.required === 1 ? "" : "s"}${n.params.variadic ? ", variádico" : ""})` : "—";
    const children = (d.children || []).slice(0, 60).map((c) => `<div class="ev exact"><div class="h"><span class="rel">${esc(c.kind)}</span><span class="t" data-id="${esc(c.id)}">${esc(c.label)}</span></div><div class="s">${esc(c.signature)} · ${esc(c.coords)}</div></div>`).join("");
    $("#tab-geral").innerHTML = `
      ${n.signature ? `<pre class="sig">${esc(n.signature)}</pre>` : ""}
      ${n.docstring ? `<p>${esc(n.docstring)}</p>` : ""}
      <dl class="kv">
        <dt>Linhas</dt><dd><code>${n.start}–${n.end}</code> (${n.end - n.start + 1} linhas)</dd>
        <dt>Arquivo</dt><dd><code>${esc(n.file)}</code></dd>
        <dt>Linguagem</dt><dd>${esc(n.language)}</dd>
        <dt>Complexidade</dt><dd>${n.complexity || "—"}</dd>
        <dt>Visibilidade</dt><dd>${esc(n.visibility || "—")}</dd>
        <dt>Parâmetros</dt><dd>${params}</dd>
        ${n.type_ref ? `<dt>Tipo</dt><dd><code>${esc(n.type_ref)}</code></dd>` : ""}
        ${d.parent ? `<dt>Declarado em</dt><dd><span class="t link" data-id="${esc(d.parent.id)}">${esc(d.parent.label)}</span></dd>` : ""}
      </dl>
      ${caps ? `<div class="section-title">Capacidades</div><div class="tags">${caps}</div>` : ""}
      ${attrs ? `<div class="section-title">Atributos / decorators</div><div class="tags">${attrs}</div>` : ""}
      ${bases ? `<div class="section-title">Herda / implementa</div><div class="tags">${bases}</div>` : ""}
      ${(d.mutations || []).length ? `<div class="section-title">Mutações de estado</div>${d.mutations.map((m) => `<div class="s"><span class="tag">${esc(m.type)}</span> ${esc(m.target)} <span class="muted">:${m.line}</span></div>`).join("")}` : ""}
      ${children ? `<div class="section-title">Membros</div>${children}` : ""}`;

    const outs = d.outgoing.map((e) => evidenceCard(e, "out")).join("");
    const ins = d.incoming.map((e) => evidenceCard(e, "in")).join("");
    $("#tab-evidencias").innerHTML = `
      <div class="section-title">Saída (${d.outgoing.length})</div>${outs || '<p class="muted">Nenhuma relação de saída.</p>'}
      <div class="section-title">Entrada (${d.incoming.length})</div>${ins || '<p class="muted">Nenhuma referência de entrada.</p>'}`;
    $("#tab-fluxos").innerHTML = '<p class="muted">Carregando…</p>';
    $("#tab-fluxos").dataset.loaded = "";
    $("#tab-dados").dataset.loaded = "";
    $("#tab-acoes").innerHTML = `
      <div class="actions">
        <button class="primary" data-act="coords">📋 Copiar coordenadas para IA <code>${esc(n.coords)}</code></button>
        <button data-act="id">Copiar ID do símbolo <code>${esc(n.id)}</code></button>
        <button data-act="body">Copiar corpo do símbolo (linhas numeradas)</button>
        <button data-act="skeleton">Copiar esqueleto do arquivo (zero-body stub)</button>
        <button data-act="impact">Calcular raio de impacto</button>
        <button data-act="tests">Testes que cobrem este símbolo</button>
        <button data-act="expand">Expandir vizinhança (2 níveis)</button>
      </div>
      <pre class="tool" id="action-out" hidden></pre>`;
    const active = document.querySelector(".tab.active")?.dataset.tab || "geral";
    setTab(active);
    const node = cy.getElementById(id);
    if (node.length) {
      cy.elements().unselect();
      node.select();
    }
  }

  async function renderFlows() {
    const page = $("#tab-fluxos");
    if (!state.detail || page.dataset.loaded === state.selected) return;
    page.dataset.loaded = state.selected;
    const n = state.detail.node;
    const parts = [];
    const hasHttp = (state.detail.routes || []).length || state.detail.outgoing.some((e) => e.relation === "HTTP_CALLS") || state.detail.incoming.some((e) => e.relation === "HTTP_CALLS");
    if (hasHttp) {
      const r = await tool("trace_fullstack_flow", { api_route_or_call: n.id, max_token_budget: 1500 });
      parts.push(`<div class="section-title">Fullstack (frontend ↔ backend)</div><pre class="tool">${esc(r.text)}</pre>`);
    }
    const h = await tool("get_call_hierarchy", { symbol_id: n.id, direction: "both", depth: 2, max_token_budget: 1500 });
    parts.push(`<div class="section-title">Hierarquia de chamadas</div><pre class="tool">${esc(h.text)}</pre>`);
    if ((n.capabilities || []).includes("THROWS")) {
      const thrown = state.detail.outgoing.filter((e) => e.relation === "THROWS").map((e) => (e.other ? e.other.label : e.unresolved_symbol));
      for (const t of [...new Set(thrown)].slice(0, 2)) {
        const r = await tool("trace_error_flow", { exception_name_or_message: t, max_token_budget: 800 });
        parts.push(`<div class="section-title">Fluxo de erro: ${esc(t)}</div><pre class="tool">${esc(r.text)}</pre>`);
      }
    }
    if (state.selected === n.id) page.innerHTML = parts.join("");
  }

  async function renderData() {
    const page = $("#tab-dados");
    if (!state.detail || page.dataset.loaded === state.selected) return;
    page.dataset.loaded = state.selected;
    const d = state.detail;
    if (d.data_model) {
      const rows = d.data_model.fields
        .map((f) => `<tr><td>${f.pk ? '<span class="key">PK</span> ' : ""}${f.fk ? '<span class="key">FK</span> ' : ""}${esc(f.name)}</td><td><code>${esc(f.type)}</code></td><td>${f.target ? esc(f.target) : ""}</td><td>${esc(f.cardinality || "")}</td></tr>`)
        .join("");
      page.innerHTML = `<p class="muted">Entidade ${esc(d.data_model.framework || "")}</p>
        <table class="fields"><thead><tr><th>Campo</th><th>Tipo</th><th>Referência</th><th>Card.</th></tr></thead><tbody>${rows}</tbody></table>`;
      return;
    }
    const kind = d.node.kind;
    if (["class", "interface", "struct", "enum", "type_alias"].includes(kind)) {
      const r = await tool("get_type_contract", { type_id: d.node.id });
      page.innerHTML = `<div class="section-title">Contrato público</div><pre class="tool">${esc(r.text)}</pre>`;
    } else {
      page.innerHTML = '<p class="muted">Este nó não é uma entidade de dados nem um tipo. Selecione uma classe/modelo para ver seus campos e relacionamentos.</p>';
    }
  }

  $("#panel").addEventListener("click", async (ev) => {
    const link = ev.target.closest("[data-id]");
    if (link) {
      const id = link.dataset.id;
      if (cy.getElementById(id).length) select(id);
      else focus(id);
      return;
    }
    const act = ev.target.closest("[data-act]")?.dataset.act;
    if (!act || !state.detail) return;
    const n = state.detail.node;
    const outEl = $("#action-out");
    const show = (t) => {
      outEl.hidden = false;
      outEl.textContent = t;
    };
    switch (act) {
      case "coords":
        return copy(n.coords, `Coordenadas copiadas: ${n.coords}`);
      case "id":
        return copy(n.id, "ID copiado");
      case "body": {
        const r = await tool("get_symbol_body", { symbol_id: n.id });
        show(r.text);
        return copy(r.text, "Corpo copiado");
      }
      case "skeleton": {
        const r = await tool("get_file_skeleton", { file_path: n.file });
        show(r.text);
        return copy(r.text, "Esqueleto copiado");
      }
      case "impact": {
        const r = await tool("get_impact_radius", { symbol_id: n.id, depth: 3, max_token_budget: 1200 });
        return show(r.text);
      }
      case "tests": {
        const r = await tool("get_tests_for_symbol", { symbol_id: n.id });
        return show(r.text);
      }
      case "expand":
        return focus(n.id, 2);
    }
  });
  $("#btn-copy").addEventListener("click", () => state.detail && copy(state.detail.node.coords, `Coordenadas copiadas: ${state.detail.node.coords}`));

  // ---------------------------------------------------------------- Search
  const input = $("#search");
  const results = $("#results");
  let searchSeq = 0;
  let selIdx = -1;
  input.addEventListener("input", async () => {
    const q = input.value.trim();
    const seq = ++searchSeq;
    if (q.length < 2) {
      results.hidden = true;
      return;
    }
    try {
      const r = await api(`/api/search?q=${encodeURIComponent(q)}&limit=25`);
      if (seq !== searchSeq) return;
      selIdx = -1;
      results.innerHTML = r.results.length
        ? r.results.map((n) => `<div class="result" data-id="${esc(n.id)}"><span class="k">${esc(n.kind)}</span><span class="n">${esc(n.label)}</span><span class="c">${esc(n.coords)}</span></div>`).join("")
        : '<div class="result"><span class="k"></span><span class="n muted">Nenhum resultado</span></div>';
      results.hidden = false;
    } catch (e) {
      toast(e.message);
    }
  });
  input.addEventListener("keydown", (ev) => {
    const items = [...results.querySelectorAll(".result[data-id]")];
    if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      selIdx = Math.max(0, Math.min(items.length - 1, selIdx + (ev.key === "ArrowDown" ? 1 : -1)));
      items.forEach((it, i) => it.classList.toggle("sel", i === selIdx));
    } else if (ev.key === "Enter") {
      const it = items[selIdx >= 0 ? selIdx : 0];
      if (it) pick(it.dataset.id);
    } else if (ev.key === "Escape") {
      results.hidden = true;
    }
  });
  results.addEventListener("click", (ev) => {
    const it = ev.target.closest(".result[data-id]");
    if (it) pick(it.dataset.id);
  });
  function pick(id) {
    results.hidden = true;
    input.blur();
    focus(id);
  }
  document.addEventListener("keydown", (ev) => {
    if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "k") {
      ev.preventDefault();
      input.focus();
      input.select();
    }
  });
  document.addEventListener("click", (ev) => {
    if (!ev.target.closest(".search")) results.hidden = true;
  });

  // ---------------------------------------------------------------- Relations
  function renderChips() {
    $("#relations").innerHTML = ALL_RELATIONS.map((r) => `<span class="chip ${state.relations.has(r) ? "on" : ""}" data-rel="${r}">${r}</span>`).join("");
  }
  $("#relations").addEventListener("click", (ev) => {
    const r = ev.target.closest("[data-rel]")?.dataset.rel;
    if (!r) return;
    if (state.relations.has(r)) state.relations.delete(r);
    else state.relations.add(r);
    renderChips();
    refresh();
  });

  function refresh() {
    return state.focus ? focus(state.focus) : loadOverview();
  }

  // ---------------------------------------------------------------- Status
  async function pollStatus() {
    try {
      const s = await api("/api/status");
      const badge = $("#status");
      const st = s.stats;
      if (s.reindexing) {
        badge.className = "badge badge-busy";
        badge.querySelector("b").textContent = `Reindexando ${s.pending_files || ""} arquivo(s)…`;
      } else if (s.stale_files.length) {
        badge.className = "badge badge-stale";
        badge.querySelector("b").textContent = `${s.stale_files.length} arquivo(s) pendente(s)`;
        badge.title = s.stale_files.slice(0, 20).join("\n");
      } else if (st.error_files) {
        badge.className = "badge badge-bad";
        badge.querySelector("b").textContent = `Atualizado · ${st.error_files} com erro`;
      } else {
        badge.className = "badge badge-ok";
        badge.querySelector("b").textContent = "Índice atualizado";
        badge.title = s.watching ? "Monitorando alterações em tempo real" : "Verificação periódica";
      }
      $("#stats").textContent = `${st.files} arquivos · ${st.nodes} nós · ${st.edges} arestas (exact ${st.exact} · inferred ${st.inferred} · unresolved ${st.unresolved})`;
      if (state.lastIndexed !== null && st.last_indexed_at !== state.lastIndexed) {
        refresh().catch(() => {});
        if (state.selected) select(state.selected);
      }
      state.lastIndexed = st.last_indexed_at;
    } catch {
      const badge = $("#status");
      badge.className = "badge badge-bad";
      badge.querySelector("b").textContent = "Servidor offline";
    }
  }

  $("#btn-overview").addEventListener("click", () => loadOverview());
  $("#btn-reindex").addEventListener("click", async () => {
    toast("Reindexando…");
    try {
      const r = await api("/api/reindex", { method: "POST" });
      toast(`${r.files_parsed} arquivo(s) processado(s) em ${r.duration_ms} ms`);
      refresh();
    } catch (e) {
      toast(e.message);
    }
  });

  renderChips();
  loadOverview().catch((e) => toast(e.message));
  pollStatus();
  setInterval(pollStatus, 1500);
})();
