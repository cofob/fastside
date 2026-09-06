# Deployment

## Prerequisites

* Rust (toolchain pinned in `rust-toolchain.toml`).
* A `services.json` file (download or generate with actualizer).
* Optional: `config.yml` for fine-tuning.

## Local run

```bash
cargo run -p fastside -- serve --services ./services.json --listen 127.0.0.1:8080
```

Open http://localhost:8080 in the browser.

## Docker

An example multi-stage build:
```dockerfile
FROM rust:1.97.1 as build
WORKDIR /code
COPY . .
RUN cargo build --release -p fastside

FROM debian:bookworm-slim
COPY --from=build /code/target/release/fastside /usr/local/bin/fastside
COPY services.json /
EXPOSE 8080
ENTRYPOINT ["fastside", "serve", "--services", "/services.json", "--listen", "0.0.0.0:8080"]
```

### Pre-built images (x86_64 & arm64)

You don’t have to build locally—the project publishes multi-arch images to GHCR:

```bash
# pull latest stable
docker pull ghcr.io/cofob/fastside:latest

# run binding 8080 and mounting config
docker run -d --name fastside \
  -p 8080:8080 \
  -v $PWD/config.yml:/config.yml:ro \
  -v $PWD/fastside-data:/data \
  ghcr.io/cofob/fastside:latest \
  --config /config.yml serve --listen 0.0.0.0:8080
```

For this volume example, set `storage.sqlite.path` to
`/data/fastside.sqlite3`. This keeps crawler and reputation data after a
container restart.

### Docker Compose

```yaml
version: "3.8"
services:
  fastside:
    image: ghcr.io/cofob/fastside:latest
    container_name: fastside
    ports:
      - "8080:8080"
    volumes:
      - ./services.json:/services.json:ro
      - ./config.yml:/config.yml:ro
      - ./fastside-data:/data
    command: [
      "--config", "/config.yml",
      "serve",
      "--services", "/services.json",
      "--listen", "0.0.0.0:8080"
    ]
    restart: unless-stopped
```

Run with `docker compose up -d`.

## Fly.io

A sample `fly.toml` is included.  Deploy with:
```bash
fly launch    # once
fly deploy    # after code changes
```

## Systemd service

```ini
[Unit]
Description=Fastside API
After=network.target

[Service]
User=fastside
WorkingDirectory=/opt/fastside
ExecStart=/usr/local/bin/fastside serve --services /opt/fastside/services.json
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

## Environment variables

| Name | Purpose |
|------|---------|
| `FS__LOG` | `error`, `warn`, `info` *(default)*, `debug`, `trace` |
| `FS__SKIP_WAIT` | Start immediately without initial crawl |

Any config field can be overridden – see `configuration.md`. 

Run `fastside validate --services services.json` to ensure schema correctness.

## Cloudflare Workers

The Worker uses the same Axum routes and redirect logic as the native server.
A Cron Trigger starts one Durable Object. Its alarm runs every two minutes and
checks 20 instances at a time. The object stores the cursor and partial results.
Workers KV stores the last complete crawler snapshot. A failed batch does not
advance the cursor, so the next alarm repeats that batch. The Durable Object is
also the authoritative writer for reputation votes. It publishes reputation
snapshots to a separate KV key. KV reads can lag, but accepted vote increments
are not lost.

```bash
nix develop
cd fastside-cloudflare
npm ci
npm run deploy
```

The Nix shell provides the LLVM compiler that Ring needs for the Wasm target.

Wrangler creates the `FASTSIDE` KV namespace and the SQLite-backed Durable Object
on the first deployment. The first alarm publishes the services as unverified
defaults. A complete snapshot replaces it after all batches finish. For local
tests, start `npm run dev`, and then run:

```bash
curl http://localhost:8787/cdn-cgi/local/scheduled
```

Set `FASTSIDE_SERVICES_URL` and `FASTSIDE_CONFIG` in `wrangler.toml` when you
need a different services source or default configuration. Set
`FASTSIDE_CRAWL_BATCH_SIZE` from 1 to 40 to change the batch size. The default of
20 leaves capacity below the free-plan limit of 50 external subrequests per
invocation.

Use `storage.backend: Auto` or `Cloudflare` in Worker configuration. Redis and
native IP vote controls are not supported on Workers. CAPTCHA verification is
supported.

The Worker supports `http://`, `https://`, `socks5://`, and `socks5h://`
crawler proxies. It uses the same tag matching and optional basic
authentication as the native server. SOCKS requests use proxy-side name
resolution. To send all current network types through one proxy, map each
network tag to that endpoint:

```yaml
proxies:
  clearnet: &crawler_proxy
    url: https://proxy.example.com:8443
    auth:
      username: fastside
      password: change-me
  tor: *crawler_proxy
  i2p: *crawler_proxy
  ygg: *crawler_proxy
```

Put the equivalent JSON in `FASTSIDE_CONFIG`. The proxy must have a public TCP
address. Workers [cannot open TCP sockets to Cloudflare IP
ranges](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/#considerations).
An HTTPS proxy must use a certificate that is valid for its host name.
