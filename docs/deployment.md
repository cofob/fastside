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
  ghcr.io/cofob/fastside:latest \
  --config /config.yml serve --listen 0.0.0.0:8080
```

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
| `FS__PING_DATA_FILE` | Path to ping data snapshot |

Any config field can be overridden – see `configuration.md`. 

Run `fastside validate --services services.json` to ensure schema correctness.

## Cloudflare Workers

The Worker uses the same Axum routes and redirect logic as the native server.
A Cron Trigger runs the crawler every five minutes. Workers KV stores the latest
crawler snapshot.

```bash
cd fastside-cloudflare
npm ci
npm run deploy
```

Wrangler creates the `FASTSIDE` KV namespace on the first deployment. The first
snapshot becomes available after the first Cron Trigger runs. For local tests,
start `npm run dev`, and then run:

```bash
curl http://localhost:8787/cdn-cgi/handler/scheduled
```

Set `FASTSIDE_SERVICES_URL` and `FASTSIDE_CONFIG` in `wrangler.toml` when you
need a different services source or default configuration. Cloudflare Workers
cannot use the native HTTP and SOCKS proxy settings. The complete bundled
services list needs a Workers Paid plan because one crawl makes more than 50
external subrequests.
