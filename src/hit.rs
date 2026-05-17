use crate::{arch, maps::MapRegion, perf::SampleData};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize)]
pub struct Hit {
    pub seq: u64,
    pub breakpoint_id: u64,
    pub pid: u32,
    pub tid: u32,
    pub timestamp_ms: u128,
    pub regs: Vec<RegisterValue>,
    pub backtrace: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegisterValue {
    pub name: &'static str,
    pub value: String,
    pub map: Option<MapRegion>,
}

#[derive(Default)]
pub struct HitFactory {
    next_seq: AtomicU64,
}

impl HitFactory {
    pub fn make_hit_with_maps(
        &self,
        breakpoint_id: u64,
        data: SampleData,
        maps: Vec<Option<MapRegion>>,
    ) -> Hit {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let regs = data
            .regs
            .iter()
            .enumerate()
            .map(|(idx, value)| RegisterValue {
                name: arch::id_to_str(idx),
                value: format!("0x{value:016x}"),
                map: maps.get(idx).cloned().unwrap_or(None),
            })
            .collect();
        let backtrace = data.backtrace.map(|frames| {
            frames
                .into_iter()
                .map(|addr| format!("0x{addr:016x}"))
                .collect()
        });

        Hit {
            seq,
            breakpoint_id,
            pid: data.pid,
            tid: data.tid,
            timestamp_ms,
            regs,
            backtrace,
        }
    }
}
