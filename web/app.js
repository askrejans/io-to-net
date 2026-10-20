/* io-to-net web dashboard.
 *
 * Data: the api/v1/stream SSE feed (status once a second, read with fetch so
 * the bearer token travels in a header), plus api/v1/{bridges/<name>/monitor,
 * ports,logs,config} on demand. Dynamic text is set with textContent only.
 * Restart and reload send X-Requested-With, which the API requires for POSTs.
 */
"use strict";
(() => {
  const $ = (id) => document.getElementById(id);
  const store = {
    get(k, d) { try { return localStorage.getItem(k) ?? d; } catch { return d; } },
    set(k, v) { try { localStorage.setItem(k, v); } catch { /* blocked */ } },
  };
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const MODES = ["bridges", "monitor", "clients", "ports", "logs", "config"];

  const S = {
    mode: MODES.includes(location.hash.slice(1)) ? location.hash.slice(1) : store.get("mode", "bridges"),
    theme: store.get("theme") || (matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark"),
    token: store.get("token", ""),
    status: null, streamUp: false, lastAt: 0,
    monitor: { bridge: store.get("monBridge", ""), after: 0, rows: [], paused: false, ascii: store.get("monAscii") === "1" },
    logs: [], ports: null, config: null,
  };

  // ── formatting ────────────────────────────────────────────────────────
  function bytes(n) {
    if (!Number.isFinite(n)) return ["—", ""];
    const u = ["B", "KiB", "MiB", "GiB"];
    let i = 0;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return [i === 0 ? String(Math.round(n)) : n.toFixed(1), u[i]];
  }
  const bytesText = (n) => bytes(n).join(" ");
  function dur(s) {
    if (!Number.isFinite(s)) return "—";
    if (s >= 86400) return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`;
    if (s >= 3600) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
    if (s >= 60) return `${Math.floor(s / 60)}m ${s % 60}s`;
    return `${s}s`;
  }
  const STATUS = { open: ["ok", "Open"], opening: ["warn", "Opening"], waiting: ["bad", "Waiting"], stopped: ["", "Stopped"] };
  const rateUnit = (b) => (["Speeduino", "Megasquirt", "Modbus RTU"].includes(b.protocol) ? "polls/s"
    : ["NMEA", "ASCII stream", "AT commands", "SCPI", "CAN-over-serial"].includes(b.protocol) ? "lines/s"
      : ["SLIP", "COBS"].includes(b.protocol) ? "frames/s" : "reads/s");

  // ── DOM helpers ───────────────────────────────────────────────────────
  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }
  function setText(id, v) { const e = typeof id === "string" ? $(id) : id; if (e && e.textContent !== v) e.textContent = v; }
  function kvInto(dl, rows) {
    const frag = document.createDocumentFragment();
    for (const [k, v, cls, small] of rows) {
      frag.append(el("dt", null, k));
      const dd = el("dd", cls || null, v);
      if (small) dd.append(el("small", null, small));
      frag.append(dd);
    }
    dl.replaceChildren(frag);
  }
  function lamp(id, state) { $(id).className = `lamp ${state}`; }
  function key(text, onClick, cls = "key small") {
    const b = el("button", cls);
    b.type = "button";
    b.append(el("i", "led"), el("span", null, text));
    b.addEventListener("click", onClick);
    return b;
  }
  function tbody(id, rows, empty) {
    const tb = $(id).tBodies[0];
    if (!rows.length) {
      const tr = el("tr"), td = el("td", "empty-note", empty);
      td.colSpan = $(id).tHead.rows[0].cells.length;
      tr.append(td);
      tb.replaceChildren(tr);
      return;
    }
    tb.replaceChildren(...rows);
  }
  function row(cells) {
    const tr = el("tr");
    for (const c of cells) {
      if (c instanceof Node) { const td = el("td"); td.append(c); tr.append(td); continue; }
      const [text, cls] = Array.isArray(c) ? c : [c, null];
      tr.append(el("td", cls, text));
    }
    return tr;
  }

  // ── API ───────────────────────────────────────────────────────────────
  const headers = (extra = {}) => ({ ...(S.token ? { Authorization: `Bearer ${S.token}` } : {}), ...extra });
  async function api(path, opts = {}) {
    const res = await fetch(path, { cache: "no-store", ...opts, headers: headers(opts.headers) });
    if (res.status === 401) { askToken(); throw new Error("unauthorized"); }
    return res;
  }
  const post = (path) => api(path, { method: "POST", headers: { "X-Requested-With": "io-to-net-dashboard" } });
  function askToken() { const d = $("auth"); if (!d.open) d.showModal(); }
  $("auth").addEventListener("close", () => {
    const t = $("auth-token").value.trim();
    if (!t) return;
    S.token = t; store.set("token", t); $("auth-token").value = "";
    restartStream(); refreshMode();
  });

  let streamCtl = null;
  async function stream() {
    let delay = 1000;
    for (;;) {
      const ctl = new AbortController();
      streamCtl = ctl;
      try {
        const res = await fetch("api/v1/stream", { headers: headers({ Accept: "text/event-stream" }), signal: ctl.signal, cache: "no-store" });
        if (res.status === 401) { setStream(false); askToken(); return; }
        if (!res.ok || !res.body) throw new Error(`HTTP ${res.status}`);
        setStream(true); delay = 1000;
        const reader = res.body.getReader(), dec = new TextDecoder();
        let buf = "";
        for (;;) {
          const { value, done } = await reader.read();
          if (done) break;
          buf += dec.decode(value, { stream: true }).replace(/\r\n/g, "\n");
          let i;
          while ((i = buf.indexOf("\n\n")) >= 0) { dispatch(buf.slice(0, i)); buf = buf.slice(i + 2); }
        }
      } catch {
        if (ctl.signal.aborted) return;
      }
      setStream(false);
      await sleep(delay); delay = Math.min(delay * 2, 15000);
    }
  }
  function restartStream() { if (streamCtl) streamCtl.abort(); stream(); }
  function setStream(up) { S.streamUp = up; renderHeader(); }
  function dispatch(block) {
    let ev = "message", data = "";
    for (const line of block.split("\n")) {
      if (line.startsWith("event:")) ev = line.slice(6).trim();
      else if (line.startsWith("data:")) data += line.slice(5).trimStart();
    }
    if (ev !== "status" || !data) return;
    try { S.status = JSON.parse(data); S.lastAt = performance.now(); } catch { return; }
    render();
  }

  // ── header + totals ───────────────────────────────────────────────────
  function renderHeader() {
    const st = S.status, live = S.streamUp && performance.now() - S.lastAt < 3000;
    lamp("lamp-link", live ? "ok" : "bad");
    if (!st) return;
    setText("version", `v${st.version}`);
    setText("uptime", dur(st.uptime_s));
    const b = st.bridges, open = b.filter((x) => x.status === "open").length;
    const clients = b.reduce((n, x) => n + x.clients.length, 0);
    lamp("lamp-sources", !b.length ? "" : open === b.length ? "ok" : open ? "warn" : "bad");
    lamp("lamp-clients", clients ? "ok" : "");
    setText("r-open", String(open));
    setText("r-open-of", `/ ${b.length}`);
    const down = b.filter((x) => x.status !== "open").map((x) => x.name);
    setText("r-open-sub", down.length ? `down: ${down.join(", ")}` : b.length ? "all sources open" : "no bridges configured");
    setText("r-clients", String(clients));
    setText("r-clients-sub", `${b.reduce((n, x) => n + x.clients_total, 0)} since start`);
    const rx = b.reduce((n, x) => n + x.rx_bps, 0), tx = b.reduce((n, x) => n + x.tx_bps, 0);
    const [rv, ru] = bytes(rx), [tv, tu] = bytes(tx);
    setText("r-rx", rv); setText("r-rx-u", `${ru}/s`);
    setText("r-tx", tv); setText("r-tx-u", `${tu}/s`);
    setText("r-rx-sub", `${bytesText(b.reduce((n, x) => n + x.rx_bytes, 0))} total`);
    setText("r-tx-sub", `${bytesText(b.reduce((n, x) => n + x.tx_bytes, 0))} total`);
    const problems = b.reduce((n, x) => n + x.open_failures + x.source_errors + x.lagged + x.auth_failures + x.tx_dropped, 0);
    setText("r-err", String(problems));
    setText("r-err-sub", problems ? "failed opens, lost sources, lagging clients, bad tokens, dropped writes" : "none");
    setText("foot-config", st.config_path ? `config: ${st.config_path}` : "—");
    setText("foot-time", new Date().toLocaleTimeString());
  }

  // ── bridges ───────────────────────────────────────────────────────────
  const cards = new Map();
  function card(name) {
    let c = cards.get(name);
    if (c) return c;
    const root = el("article", "panel bridge");
    const h2 = el("h2", null, name);
    const lampEl = el("span", "lamp"); lampEl.append(el("i"), el("span"));
    h2.append(lampEl);
    const detail = el("p", "detail"), tags = el("div", "tags"), kv = el("dl", "kv");
    const actions = el("div", "actions");
    actions.append(
      key("Restart source", async (e) => {
        const b = e.currentTarget; b.disabled = true;
        try { await post(`api/v1/bridges/${encodeURIComponent(name)}/restart`); } catch { /* shown by status */ }
        setTimeout(() => { b.disabled = false; }, 1500);
      }),
      key("Monitor", () => { S.monitor.bridge = name; store.set("monBridge", name); resetMonitor(); setMode("monitor"); }),
    );
    root.append(h2, detail, tags, kv, actions);
    c = { root, lampEl, detail, tags, kv, tagKey: "" };
    cards.set(name, c);
    return c;
  }
  function renderBridges() {
    const box = $("bridges"), list = S.status?.bridges ?? [];
    if (!list.length) {
      box.replaceChildren(el("p", "empty-note", "No bridges configured. Add [[bridges]] to the configuration file and reload."));
      cards.clear();
      return;
    }
    const names = new Set(list.map((b) => b.name));
    for (const n of [...cards.keys()]) if (!names.has(n)) cards.delete(n);
    const wanted = list.map((b) => card(b.name).root);
    if (wanted.some((r, i) => box.children[i] !== r) || box.children.length !== wanted.length) box.replaceChildren(...wanted);
    for (const b of list) {
      const c = card(b.name), [cls, label] = STATUS[b.status] ?? ["", b.status];
      c.lampEl.className = `lamp ${cls}`;
      setText(c.lampEl.lastChild, label);
      setText(c.detail, b.status === "open" ? b.detail : `${label} ${dur(b.status_for_s)} · ${b.detail}`);
      c.detail.className = `detail${b.status === "waiting" ? " bad" : ""}`;
      const tags = [[b.source_kind, true], [b.transport, true], [b.mode, false]];
      if (b.read_only) tags.push(["read-only", false]);
      if (b.auth) tags.push(["token", false]);
      if (b.frame_mode !== "none") tags.push([`frames: ${b.frame_mode.replace("_", " ")}`, false]);
      if (b.max_clients) tags.push([`max ${b.max_clients} clients`, false]);
      const tk = JSON.stringify(tags);
      if (c.tagKey !== tk) { c.tagKey = tk; c.tags.replaceChildren(...tags.map(([t, hl]) => el("span", hl ? "tag hl" : "tag", t))); }
      const [rv, ru] = bytes(b.rx_bps), [tv, tu] = bytes(b.tx_bps);
      kvInto(c.kv, [
        ["Listen", b.listen],
        ["Clients", String(b.clients.length), b.clients.length ? "hl" : null, `${b.clients_total} total`],
        ["Protocol", b.protocol, b.protocol !== "unknown" ? "hl" : null],
        ["Rate", b.frame_rate.toFixed(1), "hl", `${rateUnit(b)} · avg ${Math.round(b.avg_frame_bytes)} B`],
        ["From device", `${rv} ${ru}/s`, null, `${bytesText(b.rx_bytes)} · ${b.rx_chunks} reads`],
        ["To device", `${tv} ${tu}/s`, null, `${bytesText(b.tx_bytes)} · ${b.tx_chunks} writes`],
        ["Source", `${b.opens} opens`, b.open_failures || b.source_errors ? "warn" : null, `${b.open_failures} failed · ${b.source_errors} lost`],
        ["Client issues", String(b.refused + b.lagged + b.auth_failures + b.tx_dropped), b.lagged || b.auth_failures ? "warn" : null,
          `${b.refused} refused · ${b.lagged} lagged · ${b.replaced} replaced · ${b.auth_failures} bad token · ${b.tx_dropped} dropped`],
      ]);
    }
  }

  // ── clients ───────────────────────────────────────────────────────────
  function renderClients() {
    const rows = [];
    for (const b of S.status?.bridges ?? []) for (const c of b.clients) {
      rows.push(row([b.name, [c.addr, "id"], [dur(c.connected_s), "n"], [bytesText(c.sent_bytes), "n"], [bytesText(c.received_bytes), "n"]]));
    }
    tbody("clients-table", rows, "No clients connected.");
  }

  // ── monitor ───────────────────────────────────────────────────────────
  function resetMonitor() { S.monitor.after = 0; S.monitor.rows = []; }
  function renderMonitorKeys() {
    const list = S.status?.bridges ?? [], box = $("mon-bridges");
    if (!S.monitor.bridge && list.length) { S.monitor.bridge = list[0].name; setTimeout(pollMonitor, 0); }
    const sig = list.map((b) => b.name).join("\n") + "|" + S.monitor.bridge;
    if (box.dataset.sig === sig) return;
    box.dataset.sig = sig;
    box.replaceChildren(...list.map((b) => {
      const k = key(b.name, () => { S.monitor.bridge = b.name; store.set("monBridge", b.name); resetMonitor(); box.dataset.sig = ""; renderMonitorKeys(); pollMonitor(); });
      k.classList.toggle("active", b.name === S.monitor.bridge);
      return k;
    }));
  }
  const hexToBytes = (h) => { const a = new Uint8Array(h.length / 2); for (let i = 0; i < a.length; i++) a[i] = parseInt(h.substr(i * 2, 2), 16); return a; };
  function renderMonitor() {
    renderMonitorKeys();
    const rows = S.monitor.rows.slice(-200).reverse().map((c) => {
      const tr = el("tr", c.dir);
      const t = new Date(c.ms);
      tr.append(el("td", "t", `${t.toLocaleTimeString()}.${String(t.getMilliseconds()).padStart(3, "0")}`));
      tr.append(el("td", "dir", c.dir === "rx" ? "◀ device" : "▶ client"));
      tr.append(el("td", "n", String(c.len)));
      const b = hexToBytes(c.hex);
      const text = S.monitor.ascii
        ? Array.from(b, (x) => (x >= 0x20 && x < 0x7f ? String.fromCharCode(x) : x === 10 ? "↵" : "·")).join("")
        : c.hex.replace(/(..)/g, "$1 ").trim();
      tr.append(el("td", S.monitor.ascii ? "ascii" : "bytes", text + (c.len > b.length ? " …" : "")));
      return tr;
    });
    tbody("mon-table", rows, S.monitor.bridge || !S.status ? "No traffic yet." : "No bridges.");
    setText("mon-note", S.monitor.bridge ? `${S.monitor.bridge} · newest first · up to 1 KiB per read shown` : "last 200 reads and writes");
  }
  let monBusy = false;
  async function pollMonitor() {
    if (S.mode !== "monitor" || S.monitor.paused || !S.monitor.bridge || monBusy) return;
    monBusy = true;
    try {
      const res = await api(`api/v1/bridges/${encodeURIComponent(S.monitor.bridge)}/monitor?after=${S.monitor.after}`);
      if (res.ok) {
        const d = await res.json();
        if (d.bridge === S.monitor.bridge && d.chunks.length) {
          S.monitor.rows.push(...d.chunks);
          if (S.monitor.rows.length > 400) S.monitor.rows.splice(0, S.monitor.rows.length - 400);
          S.monitor.after = d.chunks[d.chunks.length - 1].seq;
          renderMonitor();
        }
      }
    } catch { /* retried on the next tick */ }
    monBusy = false;
  }
  $("k-mon-pause").addEventListener("click", () => {
    S.monitor.paused = !S.monitor.paused;
    $("k-mon-pause").classList.toggle("active", S.monitor.paused);
    setText($("k-mon-pause").querySelector("span"), S.monitor.paused ? "Resume" : "Pause");
  });
  $("k-mon-ascii").addEventListener("click", () => {
    S.monitor.ascii = !S.monitor.ascii; store.set("monAscii", S.monitor.ascii ? "1" : "0");
    $("k-mon-ascii").classList.toggle("active", S.monitor.ascii); renderMonitor();
  });
  $("k-mon-clear").addEventListener("click", () => { S.monitor.rows = []; renderMonitor(); });

  // ── ports ─────────────────────────────────────────────────────────────
  async function loadPorts() {
    try { const r = await api("api/v1/ports"); if (r.ok) S.ports = await r.json(); } catch { /* keep last */ }
    renderPorts();
  }
  function renderPorts() {
    const rows = (S.ports ?? []).map((p) => {
      const what = [p.manufacturer, p.product].filter(Boolean).join(" / ") || "—";
      const copy = p.selector ? key("Copy", async (e) => {
        try { await navigator.clipboard.writeText(p.selector); setText(e.currentTarget.lastChild, "Copied"); } catch { /* insecure context */ }
      }) : el("span");
      return row([[p.path, "id"], p.kind, what, [p.selector ?? "—", "mono"], copy]);
    });
    tbody("ports-table", rows, S.ports ? "No serial ports found." : "Scanning…");
  }
  $("k-ports").addEventListener("click", loadPorts);

  // ── logs ──────────────────────────────────────────────────────────────
  async function loadLogs() {
    try { const r = await api("api/v1/logs?n=500"); if (r.ok) S.logs = (await r.json()).lines; } catch { /* keep last */ }
    renderLogs();
  }
  function renderLogs() {
    const f = $("log-filter").value.trim().toLowerCase();
    const lines = S.logs.filter((l) => !f || l.toLowerCase().includes(f)).slice(-300).reverse();
    const items = lines.map((l) => {
      return el("li", /\bERROR\b/.test(l) ? "err" : /\bWARN\b/.test(l) ? "warn" : null, l);
    });
    $("log").replaceChildren(...(items.length ? items : [el("li", "empty", "No log lines.")]));
  }
  $("log-filter").addEventListener("input", renderLogs);

  // ── configuration ─────────────────────────────────────────────────────
  async function loadConfig() {
    try { const r = await api("api/v1/config"); if (r.ok) S.config = await r.json(); } catch { /* keep last */ }
    renderConfig();
  }
  function flat(obj, prefix = "", out = []) {
    for (const [k, v] of Object.entries(obj ?? {})) {
      const name = prefix ? `${prefix}.${k}` : k;
      if (v && typeof v === "object" && !Array.isArray(v)) flat(v, name, out);
      else out.push([name, v == null ? "unset" : Array.isArray(v) ? JSON.stringify(v) : String(v), v == null ? "unset" : null]);
    }
    return out;
  }
  function renderConfig() {
    const st = S.status, c = S.config;
    kvInto($("cfg-file"), [
      ["File", st?.config_path ?? "—"],
      ["Version", st ? st.version : "—"],
      ["Bridges", st ? String(st.bridges.length) : "—"],
    ]);
    const links = [["health", "Health"], ["metrics", "Prometheus metrics"], ["api/v1/status", "Status (JSON)"], ["api/v1/bridges", "Bridges (JSON)"]];
    $("cfg-endpoints").replaceChildren(...links.map(([href, label]) => {
      const li = el("li"), a = el("a", null, `/${href}`);
      a.href = href; li.append(a, el("small", null, label)); return li;
    }));
    if (!c) return;
    const groups = [];
    const general = { log_level: c.log_level, http_api: c.http_api, metrics: c.metrics };
    groups.push(["General", flat(general)]);
    for (const b of c.bridges ?? []) groups.push([`Bridge ${b.name}`, flat({ ...b, name: undefined })]);
    $("cfg-groups").replaceChildren(...groups.map(([title, rows]) => {
      const s = el("section"), dl = el("dl", "kv");
      s.append(el("h3", null, title), dl);
      kvInto(dl, rows.filter(([k]) => k !== "name"));
      return s;
    }));
  }
  $("k-reload").addEventListener("click", async () => {
    setText("reload-result", "Reloading…");
    try {
      const r = await post("api/v1/reload");
      const d = await r.json();
      if (!r.ok) setText("reload-result", `Rejected, running configuration kept: ${d.error}`);
      else {
        const parts = ["started", "restarted", "stopped", "unchanged"].map((k) => `${d[k].length} ${k}`);
        setText("reload-result", `Reloaded: ${parts.join(" · ")}`);
      }
    } catch (e) { setText("reload-result", `Reload failed: ${e.message}`); }
    loadConfig();
  });

  // ── modes ─────────────────────────────────────────────────────────────
  function render() {
    renderHeader();
    if (S.mode === "bridges") renderBridges();
    else if (S.mode === "clients") renderClients();
    else if (S.mode === "monitor") renderMonitorKeys();
    else if (S.mode === "config") renderConfig();
  }
  function refreshMode() {
    if (S.mode === "ports") loadPorts();
    else if (S.mode === "logs") loadLogs();
    else if (S.mode === "config") loadConfig();
    else if (S.mode === "monitor") { renderMonitor(); pollMonitor(); }
    render();
  }
  function setMode(m) {
    S.mode = m; store.set("mode", m);
    if (location.hash.slice(1) !== m) history.replaceState(null, "", `#${m}`);
    for (const id of MODES) $(`mode-${id}`).hidden = id !== m;
    document.querySelectorAll(".key[data-mode]").forEach((k) => k.classList.toggle("active", k.dataset.mode === m));
    refreshMode();
  }
  document.querySelectorAll(".key[data-mode]").forEach((k) => k.addEventListener("click", () => setMode(k.dataset.mode)));
  window.addEventListener("hashchange", () => { const m = location.hash.slice(1); if (MODES.includes(m)) setMode(m); });
  document.addEventListener("keydown", (e) => {
    if (e.target.closest("input, dialog") || e.metaKey || e.ctrlKey || e.altKey) return;
    const i = "123456".indexOf(e.key);
    if (i >= 0) setMode(MODES[i]);
  });
  function applyTheme() {
    document.documentElement.dataset.theme = S.theme;
    document.querySelector('meta[name="theme-color"]').content = S.theme === "light" ? "#e9ecee" : "#08090a";
    setText("k-theme-t", S.theme === "light" ? "Light" : "Dark");
  }
  $("k-theme").addEventListener("click", () => { S.theme = S.theme === "light" ? "dark" : "light"; store.set("theme", S.theme); applyTheme(); });
  document.addEventListener("visibilitychange", () => { if (!document.hidden) refreshMode(); });

  applyTheme();
  $("k-mon-ascii").classList.toggle("active", S.monitor.ascii);
  setMode(S.mode);
  stream();
  setInterval(pollMonitor, 500);
  setInterval(() => { if (S.mode === "logs" && !document.hidden) loadLogs(); }, 3000);
  setInterval(renderHeader, 1000);
})();
