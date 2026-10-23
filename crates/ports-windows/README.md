# devlanes-ports-windows

Reads the TCP port ranges Windows reserves for Hyper-V, WSL, Docker Desktop and administrators, where binding fails with access denied. The daemon's port allocator keeps new blocks clear of them and `doctor` reports the ones that overlap the port pool or the proxy ports; the e2e harness uses the same parser for its port slots. On other platforms the crate is empty.

## Usage

```rust
async fn reserved_in_pool() -> Vec<u16> {
    let reading = devlanes_ports_windows::RESERVED.get().await;
    if !reading.any_read() {
        eprintln!("{:?}; try `{}`", reading.errors, devlanes_ports_windows::LIST_COMMAND);
    }
    reading.ports_within(20000, 32767).into_iter().collect()
}
```

## API

- `RESERVED`: the process-wide `Cache`, shared by every caller.
- `Cache::get`: the last `Reading`, read again after a minute, or after a few seconds when no family could be read.
- `Reading`: the sorted ranges and a `family: reason` error per family `netsh` could not list; `reserves` and `ports_within` query it.
- `read`: one fresh reading of both families.
- `parse`: the ranges in `netsh interface <family> show excludedportrange protocol=tcp` output.

## netsh

Both IPv4 and IPv6 are read from `<System32>\netsh.exe`, never a `netsh` on `PATH`, with a 10 second bound. Headers are translated on localized Windows, so only lines that start with two port numbers count.

## License

Apache-2.0
