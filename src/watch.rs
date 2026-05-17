use crate::{filter::RegFilter, perf::PerfMap};
use log::{debug, error};
use perf_event_open_sys as sys;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::task::JoinHandle;

#[derive(Clone, Debug)]
pub struct WatchConfig {
    pub pid: u32,
    pub thread: bool,
    pub type_name: String,
    pub addr_text: String,
    pub ty: u32,
    pub addr: u64,
    pub len: u64,
    pub backtrace: bool,
    pub buf_size: usize,
    pub filter: Option<RegFilter>,
}

#[derive(Debug, Serialize)]
pub struct WatchStart {
    pub threads: Vec<u32>,
}

pub struct RunningWatch {
    cancel: Arc<AtomicBool>,
    tasks: Vec<JoinHandle<()>>,
}

impl RunningWatch {
    pub fn stop(self) {
        self.cancel.store(true, Ordering::Relaxed);
        for task in self.tasks {
            task.abort();
        }
    }
}

pub fn parse_len(s: &str) -> Option<u32> {
    match s {
        "1" => Some(sys::bindings::HW_BREAKPOINT_LEN_1),
        "2" => Some(sys::bindings::HW_BREAKPOINT_LEN_2),
        "4" => Some(sys::bindings::HW_BREAKPOINT_LEN_4),
        "8" => Some(sys::bindings::HW_BREAKPOINT_LEN_8),
        "" => Some(sys::bindings::HW_BREAKPOINT_LEN_1),
        _ => None,
    }
}

pub fn parse_watchpoint_type(s: &str) -> Option<(u32, u32)> {
    if let Some(s) = s.strip_prefix("rw") {
        let len = parse_len(s)?;
        Some((sys::bindings::HW_BREAKPOINT_RW, len))
    } else if let Some(s) = s.strip_prefix('r') {
        let len = parse_len(s)?;
        Some((sys::bindings::HW_BREAKPOINT_R, len))
    } else if let Some(s) = s.strip_prefix('w') {
        let len = parse_len(s)?;
        Some((sys::bindings::HW_BREAKPOINT_W, len))
    } else if s == "x" {
        Some((
            sys::bindings::HW_BREAKPOINT_X,
            std::mem::size_of::<nix::libc::c_long>() as u32,
        ))
    } else {
        None
    }
}

pub fn parse_addr(s: &str) -> Option<u64> {
    u64::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16).ok()
}

pub fn start_watch<F>(config: WatchConfig, handle_event: F) -> anyhow::Result<(WatchStart, RunningWatch)>
where
    F: FnMut(crate::perf::SampleData) + Send + Clone + 'static,
{
    let maps = if !config.thread {
        procfs::process::Process::new(config.pid as i32)?
            .tasks()?
            .filter_map(Result::ok)
            .map(|t| {
                (
                    t.tid as u32,
                    PerfMap::new(
                        config.ty,
                        config.addr,
                        config.len,
                        t.tid,
                        config.buf_size,
                        config.backtrace,
                    ),
                )
            })
            .filter_map(|(tid, result)| match result {
                Ok(map) => Some((tid, map)),
                Err(e) => {
                    error!("perf_map_open error: {}", e);
                    None
                }
            })
            .collect::<Vec<_>>()
    } else {
        vec![(
            config.pid,
            PerfMap::new(
                config.ty,
                config.addr,
                config.len,
                config.pid as i32,
                config.buf_size,
                config.backtrace,
            )?,
        )]
    };

    if maps.is_empty() {
        anyhow::bail!("no valid perf map");
    }
    debug!("watchpoint installed on {} thread(s)", maps.len());

    let cancel = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::with_capacity(maps.len());
    let tasks = maps
        .into_iter()
        .map(|(tid, map)| {
            let filter = config.filter.clone();
            let cancel = Arc::clone(&cancel);
            let mut handle_event = handle_event.clone();
            threads.push(tid);
            tokio::spawn(async move {
                let Err(e) = map
                    .events(move |data| {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        if match &filter {
                            Some(filter) => filter.matches(&data),
                            None => true,
                        } {
                            handle_event(data);
                        }
                    })
                    .await;
                error!("error: {}", e);
            })
        })
        .collect();

    Ok((
        WatchStart { threads },
        RunningWatch {
            cancel,
            tasks,
        },
    ))
}
