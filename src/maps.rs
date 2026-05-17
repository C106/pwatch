use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize)]
pub struct MapRegion {
    pub start: String,
    pub end: String,
    pub perms: String,
    pub offset: String,
    pub dev: String,
    pub inode: u64,
    pub pathname: Option<String>,
}

#[derive(Clone, Debug)]
struct ParsedRegion {
    start: u64,
    end: u64,
    region: MapRegion,
}

#[derive(Clone, Debug)]
struct CachedMaps {
    loaded_at: Instant,
    regions: Vec<ParsedRegion>,
}

#[derive(Clone, Default)]
pub struct MapsCache {
    inner: Arc<Mutex<HashMap<u32, CachedMaps>>>,
}

impl MapsCache {
    pub fn resolve(&self, pid: u32, addr: u64) -> Option<MapRegion> {
        let now = Instant::now();
        if let Some(region) = self.resolve_cached(pid, addr, now, false) {
            return Some(region);
        }
        self.resolve_cached(pid, addr, now, true)
    }

    fn resolve_cached(&self, pid: u32, addr: u64, now: Instant, force_refresh: bool) -> Option<MapRegion> {
        let mut inner = self.inner.lock().ok()?;
        let stale = inner
            .get(&pid)
            .map(|cached| now.duration_since(cached.loaded_at) >= Duration::from_secs(1))
            .unwrap_or(true);

        if force_refresh || stale {
            if let Ok(regions) = read_maps(pid) {
                inner.insert(
                    pid,
                    CachedMaps {
                        loaded_at: now,
                        regions,
                    },
                );
            }
        }

        inner.get(&pid).and_then(|cached| {
            cached
                .regions
                .iter()
                .find(|region| region.start <= addr && addr < region.end)
                .map(|region| region.region.clone())
        })
    }
}

fn read_maps(pid: u32) -> anyhow::Result<Vec<ParsedRegion>> {
    let content = fs::read_to_string(format!("/proc/{pid}/maps"))?;
    let mut regions = Vec::new();
    for line in content.lines() {
        if let Some(region) = parse_maps_line(line) {
            regions.push(region);
        }
    }
    Ok(regions)
}

fn parse_maps_line(line: &str) -> Option<ParsedRegion> {
    let mut parts = line.split_whitespace();
    let range = parts.next()?;
    let perms = parts.next()?.to_string();
    let offset_raw = parts.next()?;
    let dev = parts.next()?.to_string();
    let inode = parts.next()?.parse().ok()?;
    let pathname = parts.next().map(|first| {
        std::iter::once(first)
            .chain(parts)
            .collect::<Vec<_>>()
            .join(" ")
    });
    let (start_raw, end_raw) = range.split_once('-')?;
    let start = u64::from_str_radix(start_raw, 16).ok()?;
    let end = u64::from_str_radix(end_raw, 16).ok()?;
    let offset = u64::from_str_radix(offset_raw, 16).ok()?;

    Some(ParsedRegion {
        start,
        end,
        region: MapRegion {
            start: format!("0x{start:016x}"),
            end: format!("0x{end:016x}"),
            perms,
            offset: format!("0x{offset:x}"),
            dev,
            inode,
            pathname,
        },
    })
}
