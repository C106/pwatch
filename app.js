const state = {
  apiBase: localStorage.getItem("pwatch.apiBase") || "http://127.0.0.1:8080",
  eventSource: null,
  breakpoints: [],
  hits: [],
  processes: [],
};

const els = {
  apiBase: document.querySelector("#apiBase"),
  connectBtn: document.querySelector("#connectBtn"),
  refreshBtn: document.querySelector("#refreshBtn"),
  status: document.querySelector("#status"),
  form: document.querySelector("#breakpointForm"),
  pid: document.querySelector("#pid"),
  type: document.querySelector("#type"),
  addr: document.querySelector("#addr"),
  backtrace: document.querySelector("#backtrace"),
  bufSize: document.querySelector("#bufSize"),
  filter: document.querySelector("#filter"),
  processRefreshBtn: document.querySelector("#processRefreshBtn"),
  processSearch: document.querySelector("#processSearch"),
  processCount: document.querySelector("#processCount"),
  processList: document.querySelector("#processList"),
  breakpointCount: document.querySelector("#breakpointCount"),
  breakpointsBody: document.querySelector("#breakpointsBody"),
  hitLimit: document.querySelector("#hitLimit"),
  hitCount: document.querySelector("#hitCount"),
  hitsList: document.querySelector("#hitsList"),
  streamState: document.querySelector("#streamState"),
  clearHitsBtn: document.querySelector("#clearHitsBtn"),
  emptyTemplate: document.querySelector("#emptyTemplate"),
};

els.apiBase.value = state.apiBase;

els.connectBtn.addEventListener("click", connect);
els.refreshBtn.addEventListener("click", refreshAll);
els.processRefreshBtn.addEventListener("click", loadProcesses);
els.processSearch.addEventListener("input", debounce(loadProcesses, 250));
els.clearHitsBtn.addEventListener("click", () => {
  state.hits = [];
  renderHits();
});
els.hitLimit.addEventListener("change", loadHits);
els.form.addEventListener("submit", createBreakpoint);

renderBreakpoints();
renderHits();
renderProcesses();
connect();

function apiUrl(path) {
  return `${state.apiBase.replace(/\/+$/, "")}${path}`;
}

async function api(path, options = {}) {
  const response = await fetch(apiUrl(path), {
    ...options,
    headers: {
      Accept: "application/json",
      ...(options.body ? { "Content-Type": "application/json" } : {}),
      ...options.headers,
    },
  });

  if (!response.ok) {
    let message = `${response.status} ${response.statusText}`;
    try {
      const body = await response.json();
      if (body.error) {
        message = body.error;
      }
    } catch (_) {
      // Keep the HTTP status message.
    }
    throw new Error(message);
  }

  if (response.status === 204) {
    return null;
  }
  return response.json();
}

async function connect() {
  state.apiBase = els.apiBase.value.trim() || "http://127.0.0.1:8080";
  localStorage.setItem("pwatch.apiBase", state.apiBase);
  closeStream();
  setStatus("disconnected", "Connecting");

  try {
    await refreshAll();
    openStream();
    setStatus("connected", "Connected");
  } catch (error) {
    setStatus("disconnected", "Disconnected");
    showError(error.message);
  }
}

async function refreshAll() {
  await Promise.all([loadBreakpoints(), loadHits(), loadProcesses()]);
}

async function loadBreakpoints() {
  state.breakpoints = await api("/breakpoints");
  renderBreakpoints();
}

async function loadHits() {
  const limit = Number(els.hitLimit.value || 100);
  state.hits = await api(`/hits?limit=${encodeURIComponent(limit)}`);
  renderHits();
}

async function loadProcesses() {
  const query = els.processSearch.value.trim();
  const suffix = query
    ? `?limit=256&q=${encodeURIComponent(query)}`
    : "?limit=256";
  state.processes = await api(`/processes${suffix}`);
  renderProcesses();
}

async function createBreakpoint(event) {
  event.preventDefault();
  const payload = {
    pid: Number(els.pid.value),
    type: els.type.value,
    addr: els.addr.value.trim(),
    backtrace: els.backtrace.checked,
    buf_size: Number(els.bufSize.value || 0),
    filter: els.filter.value.trim() || null,
  };

  try {
    await api("/breakpoints", {
      method: "POST",
      body: JSON.stringify(payload),
    });
    await loadBreakpoints();
  } catch (error) {
    showError(error.message);
  }
}

async function deleteBreakpoint(id) {
  try {
    await api(`/breakpoints/${id}`, { method: "DELETE" });
    await loadBreakpoints();
  } catch (error) {
    showError(error.message);
  }
}

function openStream() {
  const source = new EventSource(apiUrl("/hits/stream"));
  state.eventSource = source;
  els.streamState.textContent = "SSE connecting";

  source.addEventListener("open", () => {
    els.streamState.textContent = "SSE live";
  });

  source.addEventListener("hit", (event) => {
    const hit = JSON.parse(event.data);
    state.hits.push(hit);
    const limit = Number(els.hitLimit.value || 100);
    if (state.hits.length > limit) {
      state.hits.splice(0, state.hits.length - limit);
    }
    renderHits();
  });

  source.addEventListener("error", () => {
    els.streamState.textContent = "SSE reconnecting";
  });
}

function closeStream() {
  if (state.eventSource) {
    state.eventSource.close();
    state.eventSource = null;
  }
  els.streamState.textContent = "SSE idle";
}

function renderBreakpoints() {
  els.breakpointCount.textContent = `${state.breakpoints.length} running`;
  els.breakpointsBody.replaceChildren();

  if (state.breakpoints.length === 0) {
    const row = document.createElement("tr");
    const cell = document.createElement("td");
    cell.colSpan = 7;
    cell.textContent = "No breakpoints";
    row.append(cell);
    els.breakpointsBody.append(row);
    return;
  }

  for (const breakpoint of state.breakpoints) {
    const row = document.createElement("tr");
    row.append(
      cell(`#${breakpoint.id}`),
      cell(breakpoint.pid),
      cell(breakpoint.type),
      codeCell(breakpoint.addr),
      cell(breakpoint.threads.join(", ")),
      cell(breakpoint.status),
    );
    const action = document.createElement("td");
    const button = document.createElement("button");
    button.type = "button";
    button.className = "danger";
    button.textContent = "Delete";
    button.addEventListener("click", () => deleteBreakpoint(breakpoint.id));
    action.append(button);
    row.append(action);
    els.breakpointsBody.append(row);
  }
}

function renderProcesses() {
  els.processCount.textContent = `${state.processes.length} processes`;
  els.processList.replaceChildren();

  if (state.processes.length === 0) {
    const empty = document.createElement("div");
    empty.className = "process-empty";
    empty.textContent = "No processes";
    els.processList.append(empty);
    return;
  }

  for (const process of state.processes) {
    const item = document.createElement("button");
    item.type = "button";
    item.className = "process-item";
    item.addEventListener("click", () => {
      els.pid.value = process.pid;
      els.addr.focus();
    });

    const title = document.createElement("div");
    title.className = "process-title";
    const name = document.createElement("strong");
    name.textContent = process.comm || process.pid;
    const pid = document.createElement("code");
    pid.textContent = String(process.pid);
    title.append(name, pid);

    const meta = document.createElement("div");
    meta.className = "process-meta";
    const command = process.cmdline?.length
      ? process.cmdline.join(" ")
      : process.exe || process.state || "";
    meta.textContent = command || "kernel thread";

    item.append(title, meta);
    els.processList.append(item);
  }
}

function renderHits() {
  els.hitCount.textContent = `${state.hits.length} events`;
  els.hitsList.replaceChildren();

  if (state.hits.length === 0) {
    els.hitsList.append(els.emptyTemplate.content.cloneNode(true));
    return;
  }

  for (const hit of [...state.hits].reverse()) {
    const card = document.createElement("article");
    card.className = "hit-card";

    const top = document.createElement("div");
    top.className = "hit-top";
    const title = document.createElement("div");
    title.className = "hit-title";
    title.append(
      textSpan(`#${hit.seq}`),
      textSpan(`bp ${hit.breakpoint_id}`),
      textSpan(`pid ${hit.pid}`),
      textSpan(`tid ${hit.tid}`),
    );
    const meta = document.createElement("div");
    meta.className = "hit-meta";
    meta.textContent = new Date(Number(hit.timestamp_ms)).toLocaleString();
    top.append(title, meta);

    const regs = document.createElement("div");
    regs.className = "reg-grid";
    for (const reg of hit.regs) {
      regs.append(renderReg(reg));
    }

    card.append(top, regs);
    if (hit.backtrace?.length) {
      const trace = document.createElement("div");
      trace.className = "map-path";
      trace.textContent = `backtrace: ${hit.backtrace.join(" -> ")}`;
      card.append(trace);
    }
    els.hitsList.append(card);
  }
}

function renderReg(reg) {
  const box = document.createElement("div");
  box.className = "reg";
  const name = document.createElement("div");
  name.className = "reg-name";
  name.textContent = reg.name;
  const value = document.createElement("code");
  value.textContent = reg.value;
  box.append(name, value);

  if (reg.map) {
    const map = document.createElement("div");
    map.className = "map-path";
    const path = reg.map.pathname || "[anonymous]";
    map.textContent = `${path} ${reg.map.perms} ${reg.map.start}-${reg.map.end}`;
    box.append(map);
  }
  return box;
}

function cell(value) {
  const td = document.createElement("td");
  td.textContent = value;
  return td;
}

function codeCell(value) {
  const td = document.createElement("td");
  const code = document.createElement("code");
  code.textContent = value;
  td.append(code);
  return td;
}

function textSpan(value) {
  const span = document.createElement("span");
  span.textContent = value;
  return span;
}

function setStatus(kind, text) {
  els.status.className = `status ${kind}`;
  els.status.textContent = text;
}

function showError(message) {
  const toast = document.createElement("div");
  toast.className = "toast";
  toast.textContent = message;
  document.body.append(toast);
  window.setTimeout(() => toast.remove(), 5000);
}

function debounce(fn, delayMs) {
  let timer = 0;
  return (...args) => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => fn(...args).catch((error) => {
      showError(error.message);
    }), delayMs);
  };
}
