// OpenDiag read-only page. Module-supplied text is only ever put into the page
// with textContent, never as HTML.
"use strict";

const token = new URLSearchParams(location.search).get("t") || "";
const $ = (id) => document.getElementById(id);

function showError(msg) {
  const e = $("error");
  e.textContent = msg || "";
  e.hidden = !msg;
}

async function api(path) {
  const r = await fetch(path, { headers: { "X-OpenDiag-Token": token }, cache: "no-store" });
  const body = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(body.error || "request failed (" + r.status + ")");
  return body;
}

function el(tag, text) {
  const n = document.createElement(tag);
  n.textContent = text;
  return n;
}

function fillDl(dl, rows) {
  dl.replaceChildren();
  for (const [k, v] of rows) {
    dl.append(el("dt", k), el("dd", v));
  }
}

async function loadInfo() {
  const i = await api("/api/info");
  $("version").textContent = "v" + i.version;
  fillDl($("info"), [["Connection", i.connection], ["Module", i.module]]);
  const b = $("banner");
  b.hidden = !(i.simulated || i.replay);
  if (i.replay) b.textContent = "REPLAY: this is a recorded session, not a car right now.";
  else if (i.simulated) b.textContent = "SIMULATED: this is a built-in virtual module, not a car.";
  $("replay-note").hidden = !i.replay;
  const id = await api("/api/ident");
  const rows = [];
  if (id.part_number) rows.push(["Part number", id.part_number]);
  if (id.software_version) rows.push(["Software", id.software_version]);
  if (id.vin) rows.push(["VIN", id.vin]);
  id.lines.forEach((l, n) => rows.push(["Line " + (n + 1), l]));
  fillDl($("ident"), rows.length ? rows : [["Identification", "none reported"]]);
}

async function loadDtcs() {
  const list = await api("/api/dtcs");
  const body = $("dtcs").tBodies[0];
  body.replaceChildren();
  $("dtc-empty").hidden = list.length !== 0;
  $("dtcs").hidden = list.length === 0;
  for (const d of list) {
    const tr = document.createElement("tr");
    tr.append(
      el("td", d.text),
      el("td", "0x" + d.status.toString(16).toUpperCase().padStart(2, "0")),
      el("td", d.flags.join(", ")),
      el("td", d.description || "")
    );
    body.append(tr);
  }
}

// Live data: one tile per measuring-block cell, each with its own scale.
let timer = null;
let busy = false;
const MAX_POINTS = 120;
const tiles = new Map(); // "group.cell" -> { values, formula, node parts }
const samples = []; // every reading this session, for the CSV download
let startedAt = 0;

const SVG_NS = "http://www.w3.org/2000/svg";

function makeTile(key, group, cell) {
  const root = document.createElement("div");
  root.className = "tile";
  const name = el("div", "Group " + group + ", cell " + cell);
  name.className = "name";
  const value = el("div", "");
  value.className = "value";
  const range = el("div", "");
  range.className = "range";
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 200 44");
  svg.setAttribute("preserveAspectRatio", "none");
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", "Recent values of group " + group + " cell " + cell);
  const line = document.createElementNS(SVG_NS, "polyline");
  svg.append(line);
  root.append(name, value, range, svg);
  $("tiles").append(root);
  const t = { values: [], name, value, range, line };
  tiles.set(key, t);
  return t;
}

function drawTile(t, formula) {
  const vs = t.values;
  const last = vs[vs.length - 1];
  let lo = Infinity;
  let hi = -Infinity;
  for (const v of vs) {
    lo = Math.min(lo, v);
    hi = Math.max(hi, v);
  }
  t.value.textContent = String(last);
  t.range.textContent =
    "formula 0x" + formula.toString(16).toUpperCase().padStart(2, "0") +
    " \u00b7 min " + lo + " \u00b7 max " + hi;
  const span = hi - lo || 1; // a flat series sits in the middle, not at the floor
  const pts = vs.map((v, i) => {
    const x = vs.length === 1 ? 0 : (i / (MAX_POINTS - 1)) * 200;
    const y = hi === lo ? 22 : 40 - ((v - lo) / span) * 36;
    return x.toFixed(1) + "," + y.toFixed(1);
  });
  t.line.setAttribute("points", pts.join(" "));
}

function setStatus(msg) {
  $("live-status").textContent = msg;
}

async function pollLive() {
  if (busy) return; // never queue requests behind a slow module
  busy = true;
  try {
    const live = await api("/api/live?groups=" + encodeURIComponent($("groups").value));
    showError("");
    const ms = Date.now() - startedAt;
    for (const g of live.groups) {
      g.cells.forEach((c, n) => {
        const raw = c.a * 256 + c.b;
        const key = g.group + "." + (n + 1);
        const t = tiles.get(key) || makeTile(key, g.group, n + 1);
        t.values.push(raw);
        if (t.values.length > MAX_POINTS) t.values.shift();
        drawTile(t, c.formula);
        samples.push([ms, g.group, n + 1, c.formula, c.a, c.b]);
      });
    }
    $("live-csv").disabled = samples.length === 0;
    setStatus(samples.length + " reading(s)");
  } catch (e) {
    showError(e.message);
    stopLive();
  } finally {
    busy = false;
  }
}

function stopLive() {
  if (timer !== null) clearInterval(timer);
  timer = null;
  $("live-toggle").textContent = "Start";
}

function startLive() {
  if (samples.length === 0) startedAt = Date.now();
  $("live-toggle").textContent = "Pause";
  pollLive();
  timer = setInterval(pollLive, Number($("interval").value));
}

function clearLive() {
  stopLive();
  tiles.clear();
  samples.length = 0;
  $("tiles").replaceChildren();
  $("live-csv").disabled = true;
  setStatus("");
}

function downloadCsv() {
  const rows = ["time_ms,group,cell,formula,a,b"].concat(samples.map((s) => s.join(",")));
  const blob = new Blob([rows.join("\n") + "\n"], { type: "text/csv" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = "opendiag-live.csv";
  a.click();
  URL.revokeObjectURL(a.href);
}

$("live-toggle").addEventListener("click", () => (timer !== null ? stopLive() : startLive()));
$("live-clear").addEventListener("click", clearLive);
$("live-csv").addEventListener("click", downloadCsv);
$("interval").addEventListener("change", () => {
  if (timer !== null) {
    stopLive();
    startLive();
  }
});
// Different groups are different series: start fresh so tiles are not mixed up.
$("groups").addEventListener("change", clearLive);
$("refresh-dtcs").addEventListener("click", () => loadDtcs().catch((e) => showError(e.message)));

loadInfo().then(loadDtcs).catch((e) => showError(e.message));
