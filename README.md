# pwatch-ui

Standalone browser dashboard for `pwatch serve`.

## Run

Start the API:

```bash
cargo run -- serve --listen 0.0.0.0:8080
```

Serve this directory from any static file server:

```bash
python3 -m http.server 5173 -d pwatch-ui
```

Open:

```text
http://127.0.0.1:5173
```

Set the API endpoint in the sidebar, for example:

```text
http://127.0.0.1:8080
```

The UI is intentionally independent from the Rust binary. It can be hosted on
another machine as long as it can reach the `pwatch serve` address.
