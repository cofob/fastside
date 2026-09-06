"""Run after building the captcha binary and Worker and installing npm dependencies.

python3 fastside-cloudflare/tests/captcha_smoke.py
"""
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request

import captcha_fixture as fixture

ROOT = Path(__file__).resolve().parents[2]
WORKER = ROOT / "fastside-cloudflare"


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def request(url):
    with urllib.request.urlopen(url, timeout=60) as response:
        return response.read().decode()


def check_snapshot(url, expected="Ok"):
    snapshot = json.loads(request(url + "/snapshot"))["crawled_data"]["CrawledServices"]["services"]
    assert len(snapshot) == 7, snapshot
    for service in snapshot.values():
        assert len(service["instances"]) == 1, (snapshot, fixture.proofs)
        status = service["instances"][0]["status"]
        assert expected in status, (snapshot, fixture.proofs)


def stop(process):
    if process is None:
        return
    process.terminate()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def main():
    fixture_server = fixture.http.server.ThreadingHTTPServer(("127.0.0.1", 0), fixture.Handler)
    proxy_server = fixture.http.server.ThreadingHTTPServer(("127.0.0.1", 0), fixture.Proxy)
    fixture.target_url = f"http://127.0.0.1:{fixture_server.server_port}"
    for server in (fixture_server, proxy_server):
        threading.Thread(target=server.serve_forever, daemon=True).start()
    token = secrets.token_urlsafe(24)
    solver_port, worker_port = free_port(), free_port()
    api = None
    dev = None
    try:
        with tempfile.TemporaryDirectory(prefix="fastside-captcha-") as directory:
            directory = Path(directory)
            with (directory / "runtime.log").open("w+") as log:
                try:
                    api = subprocess.Popen(
                        [str(ROOT / "target/debug/fastside-captcha-solver"), "--listen", f"127.0.0.1:{solver_port}"],
                        env={**os.environ, "FASTSIDE_CAPTCHA_SOLVER_TOKEN": token}, stdout=log, stderr=log,
                    )
                    entry = directory / "entry.js"
                    entry.write_text(f"""
import {{ CrawlerCoordinator as Original }} from {json.dumps(str(WORKER / 'build/index.js'))};
export class CrawlerCoordinator extends Original {{ async fetch(request) {{ return this.alarm(); }} }}
export default {{ async fetch(request, env) {{
    if (new URL(request.url).pathname === '/run') return env.CRAWLER.getByName('global').fetch('https://crawler.fastside/');
    return new Response(await env.FASTSIDE.get('snapshot'));
}} }};
""")
                    for mode in ("direct", "proxy", "unauthorized", "timeout"):
                        fixture.proofs.clear()
                        fixture.requests.clear()
                        fixture.generation = 1
                        fixture.response_delay = 0.2 if mode == "timeout" else 0
                        config = {"crawler": {
                            "request_timeout": {"secs": 5, "nanos": 0},
                            "max_concurrent_requests": 1,
                        }}
                        if mode == "timeout":
                            config["crawler"]["request_timeout"] = {"secs": 0, "nanos": 50_000_000}
                        if mode == "proxy":
                            config["proxies"] = {"clearnet": {
                                "url": f"http://127.0.0.1:{proxy_server.server_port}",
                            }}
                        wrangler = {
                            "name": "captcha-smoke",
                            "main": str(entry),
                            "compatibility_date": "2026-08-17",
                            "compatibility_flags": ["no_nodejs_global_timers"],
                            "kv_namespaces": [{"binding": "FASTSIDE", "id": "local"}],
                            "durable_objects": {"bindings": [{
                                "name": "CRAWLER", "class_name": "CrawlerCoordinator",
                            }]},
                            "migrations": [{
                                "tag": "v1", "new_sqlite_classes": ["CrawlerCoordinator"],
                            }],
                            "vars": {
                                "FASTSIDE_SERVICES_URL": fixture.target_url + "/services.json",
                                "FASTSIDE_CONFIG": json.dumps(config),
                                "FASTSIDE_CRAWL_BATCH_SIZE": "8",
                                "FASTSIDE_CAPTCHA_SOLVER_URL": f"http://127.0.0.1:{solver_port}/v1/solve",
                                "FASTSIDE_CAPTCHA_SOLVER_TOKEN": (
                                    "wrong-test-token" if mode == "unauthorized" else token
                                ),
                            },
                        }
                        path = directory / "wrangler.json"
                        path.write_text(json.dumps(wrangler))
                        dev = subprocess.Popen(
                            [
                                "node", str(WORKER / "node_modules/wrangler/bin/wrangler.js"),
                                "dev", "--config", str(path), "--port", str(worker_port),
                                "--ip", "127.0.0.1", "--persist-to", str(directory / mode),
                                "--log-level", "info",
                            ],
                            cwd=WORKER, stdout=log, stderr=log,
                        )
                        url = f"http://127.0.0.1:{worker_port}"
                        for _ in range(200):
                            if dev.poll() is not None:
                                raise RuntimeError("Workers runtime exited")
                            try:
                                request(url + "/snapshot")
                                break
                            except (urllib.error.URLError, ConnectionError):
                                time.sleep(0.1)
                        else:
                            raise RuntimeError("Workers runtime did not start")
                        for _ in range(2):  # Initialize, then crawl.
                            request(url + "/run")
                        if mode in ("direct", "proxy"):
                            check_snapshot(url)
                            assert fixture.proofs == {a: 1 for a in fixture.algorithms}, fixture.proofs
                            request(url + "/run")
                            check_snapshot(url)
                            assert fixture.proofs == {a: 1 for a in fixture.algorithms}, fixture.proofs
                            fixture.generation += 1  # The server rejects the saved cookies.
                            request(url + "/run")
                            check_snapshot(url)
                            assert fixture.proofs == {a: 2 for a in fixture.algorithms}, fixture.proofs
                            print(f"{mode}: all seven methods passed; sessions reused and renewed after rejection")
                        else:
                            check_snapshot(url, "RequestError" if mode == "unauthorized" else "TimedOut")
                            assert not fixture.proofs, fixture.proofs
                            print(f"{mode}: failed checks remain in the snapshot with the correct status")
                        stop(dev)
                        dev = None
                except Exception:
                    log.flush()
                    log.seek(0)
                    print(log.read()[-12000:].replace(token, "[test token]"))
                    print(fixture.requests)
                    raise
    finally:
        stop(dev)
        stop(api)
        fixture_server.shutdown()
        proxy_server.shutdown()


if __name__ == "__main__":
    main()
