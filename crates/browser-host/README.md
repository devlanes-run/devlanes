# devlanes-browser-host

The native messaging host of the devlanes browser extension. Chrome starts the devlanes binary with the extension's origin, and `bins/devlanes` hands the arguments to this crate, which relays a few requests to the daemon through `devlanes-client`.

## Usage

```sh
devlanes chrome-extension://pggffblellmckocpflaaeligadjpkahj/
```

Chrome runs that command line itself, from the host manifest. The binary checks for it before parsing the CLI:

```rust,ignore
if std::env::args_os()
    .nth(1)
    .is_some_and(|arg| devlanes_browser_host::is_origin(&arg))
{
    return devlanes_browser_host::run(std::env::args_os().skip(1));
}
```

## API

- `run(args)` serves one extension port on stdin and stdout until the extension closes it.
- `is_origin(arg)` says whether the first argument is a Chrome extension origin.
- `view::compose` builds the popup's view from `status {all: true}`, `hosts {all: true}` and `host.list`.
- `origin::ALLOWED_IDS` lists the extension ids the host answers.

## Protocol

Every message is a 32-bit length in native byte order, then UTF-8 JSON, as Chrome frames them. The extension sends `hello` first, then `watch`, `use` and `stop_and_use`, and the host refuses anything else. A request over 64 KiB closes the port, and the host never sends a view over 1 MiB.

`tests/fixtures/` holds the canonical messages and views. The browser repo keeps a copy and tests its parsers on it.

## Daemon

The host connects with `Client::connect` and never starts a daemon. While none runs it says so and tries again every 2 seconds. Its subscription is quiet, so an open browser doesn't keep the daemon past its idle exit. A daemon without the `quiet_subscribe` and `hosts_all` features reads as outdated.

Chrome starts the host with the browser's environment, so a `DEVLANES_HOME` set only in a shell doesn't reach it.

## License

Apache-2.0
