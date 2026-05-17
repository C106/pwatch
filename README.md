# pwatch

A cli tool to install a hardware breakpoint/watchpoint on a process in linux. This is useful for debugging a process without having to attach a debugger to it.

Now it supports x86_64 and arm64. You can use it on rooted Android devices as well.

## Usage

```
pwatch <pid> <type> <addr>
pwatch -t <tid> <type> <addr>
```
For example:
```bash
pwatch 31737 rw4 0x55fa689a90
```
This will install a read/write 4 byte watchpoint on the address `0x55fa689a90` of all threads in the process with pid `31737`.

full arguments:
```
Usage: pwatch [OPTIONS] <PID> <TYPE> <ADDR>

Arguments:
  <PID>   target pid, if thread is true, this is the tid of the target thread
  <TYPE>  watchpoint type, can be read(r), write(w), readwrite(rw) or execve(x). if it is one of r, w, rw, the watchpoint length is needed. Valid length is 1, 2, 4, 8. For example, r4 means a read watchpoint with length 4 and rw1 means a readwrite watchpoint with length 1
  <ADDR>  watchpoint address, in hex format. 0x prefix is optional

Options:
      --buf-size <BUF_SIZE>  buffer size, in power of 2. For example, 2 means 2^2 pages = 4 * 4096 bytes [default: 0]
      --timeout <TIMEOUT>    exit after this many seconds. 0 means no timeout [default: 0]
      --filter <FILTER>      register filter for hits, pcap-like. For example: 'ip == 0x1234 and ax != 0'
  -t                         whether the target is a thread or a process
  -b, --backtrace            whether to print backtrace
  -h, --help                 Print help
```

## Filter

`--filter` drops hits before printing them. It supports register comparisons joined with `and`/`or` or `&&`/`||`.

Examples:

```bash
pwatch --filter 'ip == 0x55fa689a90' 31737 x 0x55fa689a90
pwatch --filter 'ax != 0 and flags & 0x40 == 0' 31737 rw4 0x55fa689a90
pwatch --filter 'pc >= 0x7000000000 && pc < 0x7100000000' 31737 rw4 0x55fa689a90
```

Use the register names printed by pwatch, such as `ax`/`ip` on x86_64 or `x0`/`pc` on aarch64.

## Output

![output](img/output.png)
