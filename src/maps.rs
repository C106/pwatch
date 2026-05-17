use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    path::Path,
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

#[derive(Clone, Debug, Serialize)]
pub struct ResolvedAddress {
    pub expression: String,
    pub address: String,
    pub value: u64,
    pub map: Option<MapRegion>,
}

#[derive(Clone, Default)]
pub struct MapsCache {
    inner: Arc<Mutex<HashMap<u32, CachedMaps>>>,
}

impl MapsCache {
    pub fn list(&self, pid: u32) -> anyhow::Result<Vec<MapRegion>> {
        Ok(self
            .regions(pid, true)?
            .into_iter()
            .map(|region| region.region)
            .collect())
    }

    pub fn resolve(&self, pid: u32, addr: u64) -> Option<MapRegion> {
        let now = Instant::now();
        if let Some(region) = self.resolve_cached(pid, addr, now, false) {
            return Some(region);
        }
        self.resolve_cached(pid, addr, now, true)
    }

    pub fn resolve_expression(&self, pid: u32, expression: &str) -> anyhow::Result<ResolvedAddress> {
        let addr = if let Some(addr) = parse_numeric_addr(expression.trim()) {
            addr
        } else {
            self.resolve_module_offset(pid, expression)?
        };
        Ok(ResolvedAddress {
            expression: expression.to_string(),
            address: format!("0x{addr:016x}"),
            value: addr,
            map: self.resolve(pid, addr),
        })
    }

    fn resolve_module_offset(&self, pid: u32, expression: &str) -> anyhow::Result<u64> {
        let (module, offset) = parse_module_offset(expression)?;
        let regions = self.regions(pid, true)?;
        let module_lower = module.to_ascii_lowercase();
        let region = regions
            .iter()
            .filter(|region| region.region.pathname.as_deref().is_some())
            .find(|region| {
                let path = region.region.pathname.as_deref().unwrap_or_default();
                let lower = path.to_ascii_lowercase();
                lower == module_lower
                    || lower.ends_with(&format!("/{module_lower}"))
                    || Path::new(path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.eq_ignore_ascii_case(module))
            })
            .ok_or_else(|| anyhow::anyhow!("module not found in pid {pid} maps: {module}"))?;
        region
            .start
            .checked_add(offset)
            .ok_or_else(|| anyhow::anyhow!("resolved address overflow: {expression}"))
    }

    fn resolve_cached(&self, pid: u32, addr: u64, now: Instant, force_refresh: bool) -> Option<MapRegion> {
        self.regions_with(now, pid, force_refresh).ok().and_then(|regions| {
            regions
                .iter()
                .find(|region| region.start <= addr && addr < region.end)
                .map(|region| region.region.clone())
        })
    }

    fn regions(&self, pid: u32, force_refresh: bool) -> anyhow::Result<Vec<ParsedRegion>> {
        self.regions_with(Instant::now(), pid, force_refresh)
    }

    fn regions_with(
        &self,
        now: Instant,
        pid: u32,
        force_refresh: bool,
    ) -> anyhow::Result<Vec<ParsedRegion>> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("maps cache lock poisoned"))?;
        let stale = inner
            .get(&pid)
            .map(|cached| now.duration_since(cached.loaded_at) >= Duration::from_secs(1))
            .unwrap_or(true);

        if force_refresh || stale {
            inner.insert(
                pid,
                CachedMaps {
                    loaded_at: now,
                    regions: read_maps(pid)?,
                },
            );
        }

        Ok(inner
            .get(&pid)
            .map(|cached| cached.regions.clone())
            .unwrap_or_default())
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

fn parse_module_offset(expression: &str) -> anyhow::Result<(&str, u64)> {
    let (module, offset) = expression
        .split_once('+')
        .ok_or_else(|| anyhow::anyhow!("invalid address expression: {expression}"))?;
    let module = module.trim();
    if module.is_empty() {
        anyhow::bail!("missing module in address expression: {expression}");
    }
    let offset = parse_numeric_addr(offset.trim())
        .ok_or_else(|| anyhow::anyhow!("invalid module offset: {expression}"))?;
    Ok((module, offset))
}

fn parse_numeric_addr(value: &str) -> Option<u64> {
    if value.is_empty() {
        return None;
    }
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).ok()
    } else if value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        u64::from_str_radix(value, 16).ok()
    } else {
        None
    }
}
