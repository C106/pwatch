use crate::{
    arch,
    maps::{AddressResolution, MapRegion},
    perf::SampleData,
};
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
    pub backtrace_frames: Option<Vec<AddressValue>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegisterValue {
    pub name: &'static str,
    pub value: String,
    pub map: Option<MapRegion>,
    pub resolved: Option<AddressResolution>,
    pub display: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AddressValue {
    pub value: String,
    pub resolved: Option<AddressResolution>,
    pub display: String,
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
        reg_resolutions: Vec<Option<AddressResolution>>,
        backtrace_resolutions: Option<Vec<Option<AddressResolution>>>,
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
                map: reg_resolutions
                    .get(idx)
                    .cloned()
                    .flatten()
                    .map(|resolved| resolved.region),
                resolved: reg_resolutions.get(idx).cloned().unwrap_or(None),
                display: display_address(*value, reg_resolutions.get(idx).and_then(Option::as_ref)),
            })
            .collect();
        let backtrace = data
            .backtrace
            .as_ref()
            .map(|frames| frames.iter().map(|addr| format!("0x{addr:016x}")).collect());
        let backtrace_frames = data.backtrace.map(|frames| {
            frames
                .into_iter()
                .enumerate()
                .map(|(idx, addr)| {
                    let resolved = backtrace_resolutions
                        .as_ref()
                        .and_then(|resolutions| resolutions.get(idx))
                        .cloned()
                        .flatten();
                    AddressValue {
                        value: format!("0x{addr:016x}"),
                        display: display_address(addr, resolved.as_ref()),
                        resolved,
                    }
                })
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
            backtrace_frames,
        }
    }
}

fn display_address(addr: u64, resolved: Option<&AddressResolution>) -> String {
    match resolved {
        Some(resolved) => format!("0x{addr:016x} ({})", resolved.display),
        None => format!("0x{addr:016x}"),
    }
}
