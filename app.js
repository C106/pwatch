const state = {
  apiBase: localStorage.getItem("pwatch.apiBase") || "http://127.0.0.1:8080",
  eventSource: null,
  streamAbort: null,
  streamReadyTimer: 0,
  hitRenderQueued: false,
  breakpoints: [],
  hits: [],
  processes: [],
  maps: [],
  activeTab: "breakpoints",
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
  processPopup: document.querySelector("#processPopup"),
  processRefreshBtn: document.querySelector("#processRefreshBtn"),
  processSearch: document.querySelector("#processSearch"),
  processCount: document.querySelector("#processCount"),
  processList: document.querySelector("#processList"),
  tabs: document.querySelectorAll(".tab"),
  tabPanels: document.querySelectorAll(".tab-panel"),
  breakpointCount: document.querySelector("#breakpointCount"),
  breakpointsBody: document.querySelector("#breakpointsBody"),
  hitLimit: document.querySelector("#hitLimit"),
  hitCount: document.querySelector("#hitCount"),
  hitsList: document.querySelector("#hitsList"),
  streamState: document.querySelector("#streamState"),
  clearHitsBtn: document.querySelector("#clearHitsBtn"),
  mapsPid: document.querySelector("#mapsPid"),
  mapsSearch: document.querySelector("#mapsSearch"),
  mapsRefreshBtn: document.querySelector("#mapsRefreshBtn"),
  mapsCount: document.querySelector("#mapsCount"),
  mapsBody: document.querySelector("#mapsBody"),
  emptyTemplate: document.querySelector("#emptyTemplate"),
};

els.apiBase.value = state.apiBase;

els.connectBtn.addEventListener("click", connect);
els.refreshBtn.addEventListener("click", refreshAll);
els.pid.addEventListener("focus", openProcessPopup);
els.pid.addEventListener("click", openProcessPopup);
els.pid.addEventListener("input", () => {
  els.mapsPid.value = els.pid.value;
});
els.processRefreshBtn.addEventListener("click", () => loadProcesses());
els.processSearch.addEventListener("input", debounce(loadProcesses, 250));
document.addEventListener("pointerdown", closeProcessPopupOnOutsideClick);
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    closeProcessPopup();
  }
});
els.tabs.forEach((tab) => {
  tab.addEventListener("click", () => setActiveTab(tab.dataset.tab));
});
els.mapsRefreshBtn.addEventListener("click", loadMaps);
els.mapsSearch.addEventListener("input", renderMaps);
els.mapsPid.addEventListener("change", loadMaps);
els.clearHitsBtn.addEventListener("click", () => {
  state.hits = [];
  renderHits();
});
els.hitLimit.addEventListener("change", loadHits);
els.form.addEventListener("submit", createBreakpoint);

renderBreakpoints();
renderHits();
renderProcesses();
renderMaps();
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
  if (els.mapsPid.value) {
    await loadMaps();
  }
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
    ? `?limit=4096&q=${encodeURIComponent(query)}`
    : "?limit=4096";
  state.processes = await api(`/processes${suffix}`);
  renderProcesses();
}

async function loadMaps() {
  const pid = Number(els.mapsPid.value || els.pid.value || 0);
  if (!pid) {
    state.maps = [];
    renderMaps();
    return;
  }
  els.mapsPid.value = pid;
  state.maps = await api(`/processes/${pid}/maps`);
  renderMaps();
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
  closeStream();
  const source = new EventSource(apiUrl("/hits/stream"));
  state.eventSource = source;
  els.streamState.textContent = "SSE connecting";
  state.streamReadyTimer = window.setTimeout(() => {
    if (state.eventSource === source && source.readyState !== EventSource.OPEN) {
      source.close();
      state.eventSource = null;
      openFetchStream();
    }
  }, 2000);

  source.addEventListener("open", () => {
    window.clearTimeout(state.streamReadyTimer);
    els.streamState.textContent = "SSE live";
  });

  source.addEventListener("ready", () => {
    window.clearTimeout(state.streamReadyTimer);
    els.streamState.textContent = "SSE live";
  });

  source.addEventListener("hit", (event) => {
    const hit = JSON.parse(event.data);
    appendHit(hit);
  });

  source.addEventListener("error", () => {
    els.streamState.textContent = source.readyState === EventSource.CLOSED
      ? "SSE closed"
      : "SSE reconnecting";
  });
}

function closeStream() {
  window.clearTimeout(state.streamReadyTimer);
  if (state.eventSource) {
    state.eventSource.close();
    state.eventSource = null;
  }
  if (state.streamAbort) {
    state.streamAbort.abort();
    state.streamAbort = null;
  }
  els.streamState.textContent = "SSE idle";
}

async function openFetchStream() {
  const abort = new AbortController();
  state.streamAbort = abort;
  els.streamState.textContent = "SSE fetch";

  try {
    const response = await fetch(apiUrl("/hits/stream"), {
      headers: { Accept: "text/event-stream" },
      signal: abort.signal,
    });
    if (!response.ok || !response.body) {
      throw new Error(`${response.status} ${response.statusText}`);
    }
    els.streamState.textContent = "SSE live";
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";

    loop: while (true) {
      const { value, done } = await reader.read();
      if (done) {
        break;
      }
      buffer += decoder.decode(value, { stream: true });
      while (true) {
        const idx = buffer.indexOf("\n\n");
        if (idx < 0) {
          continue loop;
        }
        const frame = buffer.slice(0, idx);
        buffer = buffer.slice(idx + 2);
        handleSseFrame(frame);
      }
    }
  } catch (error) {
    if (error.name !== "AbortError") {
      els.streamState.textContent = "SSE disconnected";
      showError(`SSE stream failed: ${error.message}`);
    }
  }
}

function handleSseFrame(frame) {
  const lines = frame.split(/\r?\n/);
  const event = lines
    .find((line) => line.startsWith("event:"))
    ?.slice("event:".length)
    .trim() || "message";
  const data = lines
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice("data:".length).trimStart())
    .join("\n");

  if (event === "ready") {
    els.streamState.textContent = "SSE live";
  } else if (event === "hit" && data) {
    appendHit(JSON.parse(data));
  }
}

function appendHit(hit) {
  state.hits.push(hit);
  const limit = Number(els.hitLimit.value || 100);
  if (state.hits.length > limit) {
    state.hits.splice(0, state.hits.length - limit);
  }
  scheduleRenderHits();
}

function scheduleRenderHits() {
  if (state.hitRenderQueued) {
    return;
  }
  state.hitRenderQueued = true;
  requestAnimationFrame(() => {
    state.hitRenderQueued = false;
    renderHits();
  });
}

function renderBreakpoints() {
  els.breakpointCount.textContent = `${state.breakpoints.length} running`;
  els.breakpointsBody.replaceChildren();

  if (state.breakpoints.length === 0) {
    const row = document.createElement("tr");
    const cell = document.createElement("td");
    cell.colSpan = 8;
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
      codeCell(breakpoint.resolved_addr || breakpoint.addr),
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
      els.mapsPid.value = process.pid;
      loadMaps().catch((error) => showError(error.message));
      closeProcessPopup();
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

async function openProcessPopup() {
  els.processPopup.hidden = false;
  if (state.processes.length === 0) {
    try {
      await loadProcesses();
    } catch (error) {
      showError(error.message);
    }
  }
}

function closeProcessPopup() {
  els.processPopup.hidden = true;
}

function closeProcessPopupOnOutsideClick(event) {
  if (
    els.processPopup.hidden
    || els.processPopup.contains(event.target)
    || els.pid.contains(event.target)
  ) {
    return;
  }
  closeProcessPopup();
}

function renderMaps() {
  const query = els.mapsSearch.value.trim().toLowerCase();
  const maps = query
    ? state.maps.filter((map) => mapMatches(map, query))
    : state.maps;
  els.mapsCount.textContent = `${maps.length} regions`;
  els.mapsBody.replaceChildren();

  if (maps.length === 0) {
    const row = document.createElement("tr");
    const cell = document.createElement("td");
    cell.colSpan = 5;
    cell.textContent = els.mapsPid.value ? "No maps" : "Select a process";
    row.append(cell);
    els.mapsBody.append(row);
    return;
  }

  for (const map of maps) {
    const row = document.createElement("tr");
    row.append(
      codeCell(map.start),
      codeCell(map.end),
      cell(map.perms),
      codeCell(map.offset),
      cell(map.pathname || "[anonymous]"),
    );
    els.mapsBody.append(row);
  }
}

function mapMatches(map, query) {
  return [map.start, map.end, map.perms, map.offset, map.dev, map.pathname || ""]
    .some((value) => String(value).toLowerCase().includes(query));
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
      const frames = hit.backtrace_resolved?.length
        ? hit.backtrace_resolved.map((frame) => frame.display || frame.value)
        : hit.backtrace;
      trace.textContent = `backtrace: ${frames.join(" -> ")}`;
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
  value.textContent = reg.display || reg.value;
  if (reg.display && reg.display !== reg.value) {
    value.title = reg.value;
  }
  box.append(name, value);

  return box;
}

function setActiveTab(tabName) {
  state.activeTab = tabName;
  els.tabs.forEach((tab) => {
    tab.classList.toggle("active", tab.dataset.tab === tabName);
  });
  els.tabPanels.forEach((panel) => {
    panel.classList.toggle("active", panel.id === `${tabName}Tab`);
  });
  if (tabName === "maps" && els.mapsPid.value && state.maps.length === 0) {
    loadMaps().catch((error) => showError(error.message));
  }
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
