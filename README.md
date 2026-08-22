# Rush CLI

`rush` is a Rust terminal client for live Rush telemetry. It tails logs or APM
spans, applies server-side searches and structured filters, lets you freeze the
screen without losing incoming records, and opens the selected record in the
Rush web UI.

The first release uses Rush's existing bounded query APIs and polls a sliding
time window. It does not require a new streaming endpoint in `query-api`.

## Features

- Live logs and APM/span views in one TUI
- Server-side free-text search and field filters
- Pause/resume with an in-memory pending-record buffer
- Bounded deduplicated local history
- Selected-row detail panel and readable duration/severity formatting
- Exact web context:
  - APM rows open `/trace/<trace_id>`
  - Log rows open Explore Logs in a ±5 second window and highlight the timestamp
- Newline-delimited JSON mode for shell pipelines
- API-key auth and tenant scoping
- TOML, environment variable, and CLI configuration

## Install

```bash
cargo install --git https://github.com/RushObservability/cli.git
```

This installs the `rush` binary.

For local development from a checkout, use `cargo install --path .`.

### Releases

Pushing a version tag builds release archives for Linux x86_64, macOS Intel,
macOS Apple Silicon, and Windows x86_64, then publishes them with SHA-256
checksums to a GitHub Release. The tag must match the package version in
`Cargo.toml`.

```bash
# Cargo.toml version = "0.1.0"
git tag -a v0.1.0 -m "rush v0.1.0"
git push origin v0.1.0
```

To retry an existing tag without moving it, run
`gh workflow run release.yml --ref main -f tag=v0.1.0`.

## Authentication

Create an API key in **Rush → Settings → API Keys**, then export it. Environment
variables are preferred on shared machines so the key is not stored in shell
history or a world-readable file.

```bash
export RUSH_URL=https://rush.example.com
export RUSH_WEB_URL=https://rush.example.com
export RUSH_API_KEY='your-api-key'
export RUSH_TENANT=default
```

Rush API keys are tenant-scoped. `--tenant` sends `X-Rush-Tenant`, but it cannot
expand an API key beyond the tenant that issued it.

For local development the defaults are `http://localhost:8080` for the API and
`http://localhost:5173` for the web UI. Open tenants can be queried without a
key; locked tenants require one.

## Usage

Start a log tail:

```bash
rush tail logs
```

Tail only error logs for one service:

```bash
rush tail logs \
  --filter service_name=gateway \
  --filter severity=ERROR \
  --search 'connection timeout'
```

Tail slow APM spans:

```bash
rush tail apm \
  --filter service_name=articles \
  --filter 'duration_ns>=250000000'
```

Stream NDJSON into another command:

```bash
rush tail logs --output json --search panic | jq -r '.summary'
```

Use the same combined search syntax as the Rush Explore page:

```bash
rush tail logs --search 'service_name=gateway POST'
```

This sends `service_name=gateway` as a structured filter and searches the
remaining log text for `POST`.

Run `rush tail --help` for polling, buffer, and time-window options.

### Configure Kubernetes gateway access

Generate a kubeconfig that points standard `kubectl` at the Rush Kubernetes
access gateway:

```bash
export RUSH_KUBERNETES_GATEWAY_URL='https://kubernetes.rush.example.com/clusters/prod-us-east-1'
export RUSH_API_KEY='your-api-key'

rush kubernetes kubeconfig \
  --cluster prod-us-east-1 \
  --namespace payments > ~/.kube/rush-prod

KUBECONFIG=~/.kube/rush-prod kubectl get pods
```

The generated file uses Kubernetes' exec-credential protocol. It runs
`rush kubernetes credential` when `kubectl` needs a token, so the kubeconfig
does not contain the Rush API key. The gateway sees the original Kubernetes API
request and handles recording without changing `kubectl` stdout, stderr, stdin,
TTY behavior, or exit codes.

Use `--gateway-url` instead of `RUSH_KUBERNETES_GATEWAY_URL` when generating
configs for several clusters. If you pass `rush --config <path>`, the generated
exec-credential arguments retain that path.

Gateway URLs must use HTTPS. Plain HTTP is accepted only for `localhost` and
loopback IPs during local development because the exec credential sends your
Rush API key to that URL.

### Filter syntax

Filters support `=`, `!=`, `>`, `>=`, `<`, `<=`, and `~`. The `~` shorthand is
a contains match (`LIKE %value%`). Quote filters containing shell operators.

Useful log fields include `service_name`, `severity`, `body`, `trace_id`,
`span_id`, `resource.<attribute>`, and `log.<attribute>`. Useful APM fields
include `service_name`, `span_name`, `http_method`, `http_path`,
`http_status_code`, `duration_ns`, `status`, `trace_id`, and `span_id`.

## TUI controls

| Key | Action |
| --- | --- |
| `Space` | Pause/resume the visible stream; polling continues into a buffer |
| `Tab` | Switch between logs and APM |
| `/` | Edit combined structured filters and free-text search |
| `f` | Edit the most recent structured field filter; creates one when none exists |
| `x` | Remove the last field filter |
| `c` | Clear search and filters |
| `r` | Refresh now |
| `j` / `k` | Move selection |
| `g` / `G` | Jump to newest/oldest |
| `Enter` | Toggle record context |
| `w` | Toggle main-stream message wrapping (up to three lines per row) |
| `o` | Open selected context in the Rush web UI |
| `?` | Show keyboard help |
| `q` | Quit |

While editing a search or filter, use `Left` / `Right` to move the cursor,
`Home` / `End` to jump, and `Backspace` / `Delete` to edit. `Enter` applies the
new value and `Esc` cancels it. The editor also supports `Ctrl-A`, `Ctrl-E`,
`Ctrl-U`, and `Ctrl-K`.

Selected-record details always word-wrap. Main-stream rows stay compact by
default; press `w` when you want to expand messages in place.

## Configuration

The default config location is platform-specific: Linux normally uses
`~/.config/rush/config.toml`, while macOS uses the application-support directory.
Pass `--config <path>` when you want an explicit location. Copy
[`config.example.toml`](config.example.toml) as a starting point, and restrict
its permissions if it contains an API key.

Precedence is:

1. CLI options
2. `RUSH_*` environment variables
3. TOML config
4. built-in defaults

Available environment variables:

- `RUSH_URL`
- `RUSH_WEB_URL`
- `RUSH_API_KEY`
- `RUSH_TENANT`
- `RUSH_POLL_INTERVAL_MS`
- `RUSH_WINDOW_SECONDS`
- `RUSH_BUFFER_SIZE`
- `RUSH_KUBERNETES_GATEWAY_URL`

## How live tail works

The client polls newest-first results from:

- `POST /api/v1/logs` with `slim: true`
- `POST /api/v1/query` with `columns: "list"`

Each request covers a sliding recent window. The client deduplicates records,
sorts them by nanosecond timestamp, and caps memory at `buffer_size`. While the
screen is paused, network polling continues and unique incoming records are kept
in a second bounded buffer. Resume merges that buffer into the visible stream.

This approach works with current Rush deployments. A future SSE/WebSocket tail
endpoint could reduce polling overhead without changing the TUI model.

## Development

Run the complete local pull-request gate:

```bash
make ci
```

Useful targets include `make test`, `make lint`, `make build`, `make release`,
and `make install`. Run `make help` for the complete list. To start the TUI
through Cargo, use `make run-logs` or `make run-apm`; pass CLI options with
`ARGS`, for example:

```bash
make run-logs ARGS="--search 'service_name=gateway POST'"
```
