use clap::{Parser, Subcommand};
use colored::Colorize;
use log::error;
use perf::SampleData;
use std::{net::SocketAddr, time::Duration};
use watch::WatchConfig;

mod arch;
mod filter;
mod hit;
mod maps;
mod perf;
mod server;
mod watch;

#[derive(Parser)]
#[command(author, version, about)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long, default_value = "0")]
    /// buffer size, in power of 2. For example, 2 means 2^2 pages = 4 * 4096 bytes.
    buf_size: usize,
    #[arg(short)]
    /// whether the target is a thread or a process.
    thread: bool,
    #[arg(short, long)]
    /// whether to print backtrace.
    backtrace: bool,
    #[arg(long, default_value = "0")]
    /// exit after this many seconds. 0 means no timeout.
    timeout: u64,
    #[arg(long)]
    /// register filter for hits, pcap-like. For example: 'ip == 0x1234 and ax != 0'.
    filter: Option<String>,

    /// target pid, if thread is true, this is the tid of the target thread.
    pid: Option<u32>,
    /// watchpoint type, can be read(r), write(w), readwrite(rw) or execve(x).
    /// if it is one of r, w, rw, the watchpoint length is needed. Valid length is 1, 2, 4, 8.
    /// For example, r4 means a read watchpoint with length 4 and rw1 means a readwrite watchpoint with length 1.
    r#type: Option<String>,
    /// watchpoint address, in hex format. 0x prefix is optional.
    addr: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Install a hardware breakpoint/watchpoint and print hits.
    Watch(WatchCommand),
    /// Start the HTTP API server.
    Serve(ServeCommand),
}

#[derive(Parser)]
struct WatchCommand {
    #[arg(long, default_value = "0")]
    /// buffer size, in power of 2. For example, 2 means 2^2 pages = 4 * 4096 bytes.
    buf_size: usize,
    #[arg(short)]
    /// whether the target is a thread or a process.
    thread: bool,
    #[arg(short, long)]
    /// whether to print backtrace.
    backtrace: bool,
    #[arg(long, default_value = "0")]
    /// exit after this many seconds. 0 means no timeout.
    timeout: u64,
    #[arg(long)]
    /// register filter for hits, pcap-like. For example: 'ip == 0x1234 and ax != 0'.
    filter: Option<String>,
    /// target pid, if thread is true, this is the tid of the target thread.
    pid: u32,
    /// watchpoint type.
    r#type: String,
    /// watchpoint address, in hex format. 0x prefix is optional.
    addr: String,
}

#[derive(Parser)]
struct ServeCommand {
    #[arg(long, default_value = "0.0.0.0:8080")]
    /// listen address for the HTTP API.
    listen: SocketAddr,
    #[arg(long, default_value = "1024")]
    /// number of recent hits retained for GET /hits.
    hit_buffer: usize,
}

impl Args {
    fn into_mode(self) -> anyhow::Result<Mode> {
        match self.command {
            Some(Command::Serve(command)) => Ok(Mode::Serve(command)),
            Some(Command::Watch(command)) => Ok(Mode::Watch(command)),
            None => Ok(Mode::Watch(WatchCommand {
                buf_size: self.buf_size,
                thread: self.thread,
                backtrace: self.backtrace,
                timeout: self.timeout,
                filter: self.filter,
                pid: self
                    .pid
                    .ok_or_else(|| anyhow::anyhow!("missing required argument: <PID>"))?,
                r#type: self
                    .r#type
                    .ok_or_else(|| anyhow::anyhow!("missing required argument: <TYPE>"))?,
                addr: self
                    .addr
                    .ok_or_else(|| anyhow::anyhow!("missing required argument: <ADDR>"))?,
            })),
        }
    }
}

enum Mode {
    Watch(WatchCommand),
    Serve(ServeCommand),
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();
    match Args::parse().into_mode()? {
        Mode::Watch(command) => run_watch(command).await,
        Mode::Serve(command) => {
            server::serve(server::ServerConfig {
                listen: command.listen,
                hit_buffer: command.hit_buffer,
            })
            .await
        }
    }
}

async fn run_watch(command: WatchCommand) -> anyhow::Result<()> {
    let (ty, bp_len) = watch::parse_watchpoint_type(&command.r#type)
        .ok_or_else(|| anyhow::anyhow!(format!("invalid watchpoint type: {}", command.r#type)))?;
    let addr = watch::parse_addr(&command.addr)
        .ok_or_else(|| anyhow::anyhow!(format!("invalid address: {}", command.addr)))?;
    let filter = command
        .filter
        .as_deref()
        .map(filter::RegFilter::parse)
        .transpose()?;
    let config = WatchConfig {
        pid: command.pid,
        thread: command.thread,
        type_name: command.r#type,
        addr_text: command.addr,
        ty,
        addr,
        len: bp_len as u64,
        backtrace: command.backtrace,
        buf_size: command.buf_size,
        filter,
    };
    let (_, running) = match watch::start_watch(config, handle_event) {
        Ok(watch) => watch,
        Err(e) if e.to_string() == "no valid perf map" => {
            error!("no valid perf map");
            return Ok(());
        }
        Err(e) => return Err(e),
    };

    if command.timeout == 0 {
        futures::future::pending::<()>().await;
    } else {
        tokio::time::sleep(Duration::from_secs(command.timeout)).await;
        running.stop().await;
    }
    Ok(())
}

fn handle_event(data: SampleData) {
    println!("-------");
    println!(
        "{}: {} {}: {}",
        "pid".yellow().bold(),
        data.pid,
        "tid".yellow().bold(),
        data.tid
    );
    for (i, reg) in data.regs.iter().enumerate() {
        print!("{:>5}: 0x{:016x} ", arch::id_to_str(i).bold().blue(), reg);
        if (i + 1) % 4 == 0 {
            println!();
        }
    }
    if data.regs.len() % 4 != 0 {
        println!();
    }
    if let Some(backtrace) = data.backtrace {
        println!("{}:", "backtrace".yellow().bold());
        for addr in backtrace {
            println!("  0x{:016x}", addr);
        }
    }
}
