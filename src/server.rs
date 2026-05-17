use crate::{
    hit::{Hit, HitFactory},
    maps::{MapRegion, MapsCache},
    watch::{self, RunningWatch, WatchConfig},
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{
        header::{
            ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
        },
        HeaderValue, Method, Request, StatusCode,
    },
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

#[derive(Clone)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub hit_buffer: usize,
}

#[derive(Clone)]
struct AppState {
    inner: Arc<InnerState>,
}

struct InnerState {
    next_breakpoint_id: AtomicU64,
    hit_factory: HitFactory,
    maps: MapsCache,
    hit_buffer: Mutex<VecDeque<Hit>>,
    hit_buffer_limit: usize,
    breakpoints: Mutex<HashMap<u64, BreakpointEntry>>,
    hit_tx: broadcast::Sender<Hit>,
}

struct BreakpointEntry {
    view: BreakpointView,
    watch: RunningWatch,
}

#[derive(Clone, Debug, Serialize)]
pub struct BreakpointView {
    pub id: u64,
    pub pid: u32,
    #[serde(rename = "type")]
    pub type_name: String,
    pub addr: String,
    pub resolved_addr: String,
    pub resolved_map: Option<MapRegion>,
    pub threads: Vec<u32>,
    pub created_at_ms: u128,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateBreakpointRequest {
    pub pid: u32,
    #[serde(rename = "type")]
    pub type_name: String,
    pub addr: String,
    #[serde(default)]
    pub backtrace: bool,
    #[serde(default)]
    pub buf_size: usize,
    pub filter: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProcessView {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub state: Option<String>,
    pub comm: String,
    pub cmdline: Vec<String>,
    pub exe: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HitsQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ProcessesQuery {
    q: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ResolveQuery {
    addr: String,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

pub async fn serve(config: ServerConfig) -> anyhow::Result<()> {
    let (hit_tx, _) = broadcast::channel(1024);
    let state = AppState {
        inner: Arc::new(InnerState {
            next_breakpoint_id: AtomicU64::new(1),
            hit_factory: HitFactory::default(),
            maps: MapsCache::default(),
            hit_buffer: Mutex::new(VecDeque::with_capacity(config.hit_buffer)),
            hit_buffer_limit: config.hit_buffer,
            breakpoints: Mutex::new(HashMap::new()),
            hit_tx,
        }),
    };

    let app = Router::new()
        .route("/breakpoints", post(create_breakpoint).get(list_breakpoints))
        .route("/breakpoints/:id", delete(delete_breakpoint))
        .route("/hits", get(get_hits))
        .route("/hits/stream", get(stream_hits))
        .route("/processes", get(list_processes))
        .route("/processes/:pid/maps", get(list_maps))
        .route("/processes/:pid/resolve", get(resolve_address))
        .with_state(state)
        .layer(middleware::from_fn(add_cors_headers));

    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn create_breakpoint(
    State(state): State<AppState>,
    Json(request): Json<CreateBreakpointRequest>,
) -> impl IntoResponse {
    match create_breakpoint_inner(state, request) {
        Ok(view) => (StatusCode::CREATED, Json(view)).into_response(),
        Err(e) => error_response(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

fn create_breakpoint_inner(
    state: AppState,
    request: CreateBreakpointRequest,
) -> anyhow::Result<BreakpointView> {
    let (ty, len) = watch::parse_watchpoint_type(&request.type_name)
        .ok_or_else(|| anyhow::anyhow!("invalid watchpoint type: {}", request.type_name))?;
    let resolved = state
        .inner
        .maps
        .resolve_expression(request.pid, &request.addr)?;
    let addr = resolved.value;
    let filter = request
        .filter
        .as_deref()
        .map(crate::filter::RegFilter::parse)
        .transpose()?;
    let id = state
        .inner
        .next_breakpoint_id
        .fetch_add(1, Ordering::Relaxed);
    let config = WatchConfig {
        pid: request.pid,
        thread: false,
        type_name: request.type_name,
        addr_text: resolved.address.clone(),
        ty,
        addr,
        len: len as u64,
        backtrace: request.backtrace,
        buf_size: request.buf_size,
        filter,
    };

    let emit_state = state.clone();
    let (start, watch) = watch::start_watch(config.clone(), move |data| {
        let hit = emit_state.inner.hit_factory.make_hit(id, data, |addr| {
            emit_state.inner.maps.resolve(config.pid, addr)
        });
        emit_state.push_hit(hit);
    })?;

    let view = BreakpointView {
        id,
        pid: config.pid,
        type_name: config.type_name,
        addr: resolved.expression,
        resolved_addr: config.addr_text,
        resolved_map: resolved.map,
        threads: start.threads,
        created_at_ms: now_ms(),
        status: "running".to_string(),
    };

    let mut breakpoints = state
        .inner
        .breakpoints
        .lock()
        .map_err(|_| anyhow::anyhow!("breakpoint lock poisoned"))?;
    breakpoints.insert(
        id,
        BreakpointEntry {
            view: view.clone(),
            watch,
        },
    );
    Ok(view)
}

async fn list_breakpoints(State(state): State<AppState>) -> impl IntoResponse {
    match state.inner.breakpoints.lock() {
        Ok(breakpoints) => {
            let mut views = breakpoints
                .values()
                .map(|entry| entry.view.clone())
                .collect::<Vec<_>>();
            views.sort_by_key(|view| view.id);
            Json(views).into_response()
        }
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "breakpoint lock poisoned"),
    }
}

async fn delete_breakpoint(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.inner.breakpoints.lock() {
        Ok(mut breakpoints) => {
            if let Some(entry) = breakpoints.remove(&id) {
                entry.watch.stop();
                StatusCode::NO_CONTENT.into_response()
            } else {
                error_response(StatusCode::NOT_FOUND, format!("breakpoint {id} not found"))
            }
        }
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "breakpoint lock poisoned"),
    }
}

async fn get_hits(
    State(state): State<AppState>,
    Query(query): Query<HitsQuery>,
) -> impl IntoResponse {
    match state.inner.hit_buffer.lock() {
        Ok(buffer) => {
            let limit = query.limit.unwrap_or(100);
            let mut hits = buffer
                .iter()
                .rev()
                .take(limit)
                .cloned()
                .collect::<Vec<_>>();
            hits.reverse();
            Json(hits).into_response()
        }
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "hit buffer lock poisoned"),
    }
}

async fn stream_hits(State(state): State<AppState>) -> impl IntoResponse {
    let stream = BroadcastStream::new(state.inner.hit_tx.subscribe()).filter_map(|result| {
        result.ok().map(|hit| {
            let data = serde_json::to_string(&hit)
                .map_err(|e| axum::Error::new(e))?;
            Ok::<_, axum::Error>(Event::default().event("hit").data(data))
        })
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn list_processes(Query(query): Query<ProcessesQuery>) -> impl IntoResponse {
    match read_processes(query) {
        Ok(processes) => Json(processes).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn list_maps(State(state): State<AppState>, Path(pid): Path<u32>) -> impl IntoResponse {
    match state.inner.maps.list(pid) {
        Ok(maps) => Json(maps).into_response(),
        Err(e) => error_response(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn resolve_address(
    State(state): State<AppState>,
    Path(pid): Path<u32>,
    Query(query): Query<ResolveQuery>,
) -> impl IntoResponse {
    match state.inner.maps.resolve_expression(pid, &query.addr) {
        Ok(resolved) => Json(resolved).into_response(),
        Err(e) => error_response(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

fn read_processes(query: ProcessesQuery) -> anyhow::Result<Vec<ProcessView>> {
    let needle = query.q.map(|q| q.to_ascii_lowercase());
    let limit = query.limit.unwrap_or(4096).min(4096);
    let mut processes = Vec::new();

    for entry in fs::read_dir("/proc")? {
        let Ok(entry) = entry else {
            continue;
        };
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let Ok(pid) = file_name.parse::<u32>() else {
            continue;
        };
        let Some(process) = read_process(pid) else {
            continue;
        };
        if let Some(needle) = &needle {
            if !process_matches(&process, needle) {
                continue;
            }
        }
        processes.push(process);
    }

    processes.sort_by_key(|process| process.pid);
    if processes.len() > limit {
        processes.truncate(limit);
    }
    Ok(processes)
}

fn read_process(pid: u32) -> Option<ProcessView> {
    let proc_dir = PathBuf::from(format!("/proc/{pid}"));
    let status = fs::read_to_string(proc_dir.join("status")).ok();
    let comm = fs::read_to_string(proc_dir.join("comm"))
        .ok()
        .map(|comm| comm.trim().to_string())
        .filter(|comm| !comm.is_empty())
        .or_else(|| status.as_deref().and_then(status_name))
        .unwrap_or_else(|| pid.to_string());
    let cmdline = fs::read(proc_dir.join("cmdline"))
        .ok()
        .map(parse_cmdline)
        .unwrap_or_default();
    let exe = fs::read_link(proc_dir.join("exe"))
        .ok()
        .map(|path| path.display().to_string());
    let ppid = status.as_deref().and_then(status_u32("PPid"));
    let state = status.as_deref().and_then(status_string("State"));

    Some(ProcessView {
        pid,
        ppid,
        state,
        comm,
        cmdline,
        exe,
    })
}

fn parse_cmdline(bytes: Vec<u8>) -> Vec<String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect()
}

fn status_name(status: &str) -> Option<String> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Name:"))
        .map(|value| value.trim().to_string())
}

fn status_u32(key: &'static str) -> impl Fn(&str) -> Option<u32> {
    move |status| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}:")))
            .and_then(|value| value.trim().parse().ok())
    }
}

fn status_string(key: &'static str) -> impl Fn(&str) -> Option<String> {
    move |status| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}:")))
            .map(|value| value.trim().to_string())
    }
}

fn process_matches(process: &ProcessView, needle: &str) -> bool {
    process.pid.to_string().contains(needle)
        || process.comm.to_ascii_lowercase().contains(needle)
        || process
            .cmdline
            .iter()
            .any(|arg| arg.to_ascii_lowercase().contains(needle))
        || process
            .exe
            .as_deref()
            .is_some_and(|exe| exe.to_ascii_lowercase().contains(needle))
}

impl AppState {
    fn push_hit(&self, hit: Hit) {
        if self.inner.hit_buffer_limit > 0 {
            if let Ok(mut buffer) = self.inner.hit_buffer.lock() {
                while buffer.len() >= self.inner.hit_buffer_limit {
                    buffer.pop_front();
                }
                buffer.push_back(hit.clone());
            }
        }
        let _ = self.inner.hit_tx.send(hit);
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn error_response(status: StatusCode, error: impl ToString) -> axum::response::Response {
    (status, Json(ErrorBody { error: error.to_string() })).into_response()
}

async fn add_cors_headers(request: Request<Body>, next: Next) -> axum::response::Response {
    let mut response = if request.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    headers.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET,POST,DELETE,OPTIONS"),
    );
    headers.insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type,accept"),
    );
    response
}
