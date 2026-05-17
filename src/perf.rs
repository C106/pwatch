use crate::arch;
use log::{debug, warn};
use nix::sys::mman::{MapFlags, ProtFlags};
use perf_event_open_sys as sys;
use std::{
    convert::Infallible,
    ffi::c_void,
    num::NonZeroUsize,
    os::fd::{FromRawFd, OwnedFd},
    ptr::NonNull,
};
use tokio::io::unix::AsyncFd;

const PERF_SAMPLE_TID: u64 = sys::bindings::PERF_SAMPLE_TID;
const PERF_SAMPLE_CALLCHAIN: u64 = sys::bindings::PERF_SAMPLE_CALLCHAIN;
const PERF_SAMPLE_REGS_USER: u64 = sys::bindings::PERF_SAMPLE_REGS_USER;
const MAX_EVENTS_PER_READINESS: usize = 128;

pub struct PerfMap {
    mmap_addr: usize,
    mmap_len: usize,
    fd: AsyncFd<OwnedFd>,
    sample_type: u64,
}

pub struct SampleData {
    pub pid: u32,
    pub tid: u32,
    pub regs: Vec<u64>,
    pub backtrace: Option<Vec<u64>>,
}

impl PerfMap {
    pub fn new(
        r#type: u32,
        addr: u64,
        len: u64,
        pid: i32,
        buf_size: usize,
        backtrace: bool,
    ) -> anyhow::Result<Self> {
        let mut attrs = sys::bindings::perf_event_attr::default();

        attrs.set_precise_ip(2);
        attrs.size = std::mem::size_of::<sys::bindings::perf_event_attr>() as u32;
        attrs.type_ = sys::bindings::PERF_TYPE_BREAKPOINT;
        attrs.__bindgen_anon_1.sample_period = 1;
        attrs.__bindgen_anon_2.wakeup_events = 1;
        attrs.bp_type = r#type;
        attrs.__bindgen_anon_3.bp_addr = addr;
        attrs.__bindgen_anon_4.bp_len = len;
        attrs.sample_type = sys::bindings::PERF_SAMPLE_REGS_USER | sys::bindings::PERF_SAMPLE_TID;
        if backtrace {
            attrs.sample_type |= sys::bindings::PERF_SAMPLE_CALLCHAIN;
            attrs.set_exclude_callchain_kernel(1);
            attrs.set_exclude_callchain_kernel(0);
        }
        attrs.sample_regs_user = arch::SAMPLE_REGS_USER;

        let perf_fd = unsafe {
            OwnedFd::from_raw_fd(nix::Error::result(sys::perf_event_open(
                &mut attrs,
                pid,
                -1,
                -1,
                (sys::bindings::PERF_FLAG_FD_CLOEXEC) as u64,
            ))?)
        };
        debug!(
            "opened perf breakpoint pid={} type={} addr=0x{:x} len={} sample_type=0x{:x}",
            pid, r#type, addr, len, attrs.sample_type
        );
        let mmap_len = (1 + (1 << buf_size)) * 4096;
        let mmap_addr = unsafe {
            nix::sys::mman::mmap(
                None,
                NonZeroUsize::new(mmap_len).unwrap(),
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED,
                &perf_fd,
                0,
            )
        }?
        .as_ptr() as usize;
        let mmap_page_metadata = unsafe {
            (mmap_addr as *mut sys::bindings::perf_event_mmap_page)
                .as_mut()
                .unwrap()
        };
        if mmap_page_metadata.compat_version != 0 {
            anyhow::bail!("unsupported mmap_page version");
        }
        Ok(Self {
            mmap_addr: mmap_addr as usize,
            mmap_len,
            fd: AsyncFd::new(perf_fd)?,
            sample_type: attrs.sample_type,
        })
    }

    pub async fn events<F: FnMut(SampleData)>(&self, mut handle: F) -> anyhow::Result<Infallible> {
        let mmap_page_metadata = unsafe {
            (self.mmap_addr as *mut sys::bindings::perf_event_mmap_page)
                .as_mut()
                .unwrap()
        };
        let data_addr = self.mmap_addr + mmap_page_metadata.data_offset as usize;
        let data_size = mmap_page_metadata.data_size as usize;
        let mut read_data_size = 0u64;
        loop {
            let mut guard = self.fd.readable().await?;
            let mut processed = 0usize;
            while mmap_page_metadata.data_head != read_data_size
                && processed < MAX_EVENTS_PER_READINESS
            {
                let mut reader = RingReader {
                    data_addr,
                    data_size,
                    base_offset: read_data_size as usize,
                    offset: 0,
                };
                let data_header = reader.read_header();
                if data_header.type_ == sys::bindings::PERF_RECORD_SAMPLE {
                    reader.offset = std::mem::size_of::<sys::bindings::perf_event_header>();
                    if let Some(data) = self.read_sample(&mut reader) {
                        handle(data);
                    }
                } else if data_header.type_ == sys::bindings::PERF_RECORD_LOST {
                    reader.offset = std::mem::size_of::<sys::bindings::perf_event_header>();
                    let lost = reader.read_u64();
                    debug!("Lost {} events", lost);
                } else {
                    debug!("Unknown perf record type: {}", data_header.type_);
                }
                read_data_size += data_header.size as u64;
                mmap_page_metadata.data_tail = read_data_size;
                processed += 1;
            }
            if mmap_page_metadata.data_head == read_data_size {
                guard.clear_ready();
            }
            drop(guard);
            if processed == MAX_EVENTS_PER_READINESS {
                tokio::task::yield_now().await;
            }
        }
    }

    fn read_sample(&self, reader: &mut RingReader) -> Option<SampleData> {
        let mut pid = 0;
        let mut tid = 0;
        let mut backtrace = None;
        let mut regs = Vec::new();

        if self.sample_type & PERF_SAMPLE_TID != 0 {
            pid = reader.read_u32();
            tid = reader.read_u32();
        }

        if self.sample_type & PERF_SAMPLE_CALLCHAIN != 0 {
            let backtrace_size = reader.read_u64();
            backtrace = Some(
                (0..backtrace_size)
                    .map(|_| reader.read_u64())
                    .collect::<Vec<_>>(),
            );
        }

        if self.sample_type & PERF_SAMPLE_REGS_USER != 0 {
            let abi = reader.read_u64();
            if abi == 0 {
                warn!("sample has no user register ABI");
                return None;
            }
            regs = vec![0u64; arch::regs_count()];
            for reg in regs.iter_mut() {
                *reg = reader.read_u64();
            }
        }

        Some(SampleData {
            pid,
            tid,
            regs,
            backtrace,
        })
    }
}

impl Drop for PerfMap {
    fn drop(&mut self) {
        let Some(addr) = NonNull::new(self.mmap_addr as *mut c_void) else {
            return;
        };
        if let Err(e) = unsafe { nix::sys::mman::munmap(addr, self.mmap_len) } {
            warn!("failed to unmap perf ring buffer: {}", e);
        }
    }
}

struct RingReader {
    data_addr: usize,
    data_size: usize,
    base_offset: usize,
    offset: usize,
}

impl RingReader {
    fn read_header(&mut self) -> sys::bindings::perf_event_header {
        sys::bindings::perf_event_header {
            type_: self.read_u32(),
            misc: self.read_u16(),
            size: self.read_u16(),
        }
    }

    fn read_u16(&mut self) -> u16 {
        u16::from_ne_bytes(self.read_array())
    }

    fn read_u32(&mut self) -> u32 {
        u32::from_ne_bytes(self.read_array())
    }

    fn read_u64(&mut self) -> u64 {
        u64::from_ne_bytes(self.read_array())
    }

    fn read_array<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        for byte in &mut out {
            let ring_offset = (self.base_offset + self.offset) % self.data_size;
            *byte = unsafe { *((self.data_addr + ring_offset) as *const u8) };
            self.offset += 1;
        }
        out
    }
}
