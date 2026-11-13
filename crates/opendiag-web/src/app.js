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
  b.hidden = !i.simulated;
  if (i.simulated) b.textContent = "SIMULATED: this is a built-in virtual module, not a car.";
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

// Live data
let timer = null;
const history = new Map(); // "group.cell" -> [values]
const MAX_POINTS = 120;

function drawChart() {
  const svg = $("chart");
  svg.replaceChildren();
  const series = [...history.values()];
  let max = 1;
  for (const s of series) for (const v of s) max = Math.max(max, v);
  for (const s of series) {
    const pts = s.map((v, i) => {
      const x = (i / (MAX_POINTS - 1)) * 600;
      const y = 155 - (v / max) * 150;
      return x.toFixed(1) + "," + y.toFixed(1);
    });
    const pl = document.createElementNS("http://www.w3.org/2000/svg", "polyline");
    pl.setAttribute("points", pts.join(" "));
    svg.append(pl);
  }
  svg.hidden = series.length === 0;
}

async function pollLive() {
  try {
    const live = await api("/api/live?groups=" + encodeURIComponent($("groups").value));
    showError("");
    const body = $("live").tBodies[0];
    body.replaceChildren();
    for (const g of live.groups) {
      g.cells.forEach((c, n) => {
        const raw = c.a * 256 + c.b;
        const key = g.group + "." + n;
        const h = history.get(key) || [];
        h.push(raw);
        if (h.length > MAX_POINTS) h.shift();
        history.set(key, h);
        const tr = document.createElement("tr");
        tr.append(el("td", String(g.group)), el("td", String(n + 1)), el("td", "0x" + c.formula.toString(16).toUpperCase().padStart(2, "0")), el("td", String(c.a)), el("td", String(c.b)), el("td", String(raw)));
        body.append(tr);
      });
    }
    $("live").hidden = false;
    drawChart();
  } catch (e) {
    showError(e.message);
    stopLive();
  }
}

function stopLive() {
  if (timer !== null) clearInterval(timer);
  timer = null;
  $("live-toggle").textContent = "Start";
}

$("live-toggle").addEventListener("click", () => {
  if (timer !== null) return stopLive();
  history.clear();
  $("live-toggle").textContent = "Stop";
  pollLive();
  timer = setInterval(pollLive, 1000);
});
$("refresh-dtcs").addEventListener("click", () => loadDtcs().catch((e) => showError(e.message)));

loadInfo().then(loadDtcs).catch((e) => showError(e.message));
