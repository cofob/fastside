"""Anubis protocol fixture and forwarding proxy for the Workers smoke test."""
import hashlib
import http.server
import json
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

vectors = json.loads(
    (Path(__file__).resolve().parents[2]
     / 'fastside-shared/tests/fixtures/anubis-vectors.json').read_text()
)
algorithms = ['fast', 'slow', 'sha256', 'argon2id', 'hashx', 'metarefresh', 'preact']
issued = {}
proofs = {}
requests = []
data = '74657374'
target_url = ''
generation = 1
response_delay = 0


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def handle(self):
        try:
            super().handle()
        except ConnectionResetError:
            pass  # A cancelled Worker request resets its connection.

    def log_message(self, *args):
        pass

    def reply(self, code, body, headers=None):
        if isinstance(body, str):
            body = body.encode()
        self.send_response(code)
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except BrokenPipeError:
            pass  # The timeout test closes its connection before the response.

    def do_GET(self):
        u = urllib.parse.urlsplit(self.path)
        q = urllib.parse.parse_qs(u.query)
        requests.append((u.path, self.headers.get('Cookie', '')))
        assert not self.headers.get('Authorization'), 'Solver token leaked to target'
        if u.path == '/services.json':
            services = [{
                'type': a,
                'test_url': '/' + a,
                'follow_redirects': a != 'fast',
                'search_string': 'protected-' + a,
                'instances': [{
                    'url': target_url + '/',
                    'tags': ['anubis', 'antibot', 'clearnet'],
                }],
            } for a in algorithms]
            self.reply(200, json.dumps({'services': services}))
            return
        if u.path == '/stats':
            self.reply(200, json.dumps(proofs))
            return
        time.sleep(response_delay)
        if u.path.endswith('/pass-challenge'):
            a = q['id'][0]
            elapsed = time.monotonic() - issued[a]
            assert 'verification=' + a in self.headers.get('Cookie', ''), self.headers
            if a in ['fast', 'slow']:
                digest = hashlib.sha256((data + q['nonce'][0]).encode()).hexdigest()
                assert q['response'][0] == digest and digest.startswith('0')
            elif a in ['sha256', 'argon2id', 'hashx']:
                v = next((v for v in vectors if v['algorithm'] == a))
                assert q['response'][0] == v['hash'] and int(q['nonce'][0]) == v['nonce']
            elif a == 'metarefresh':
                assert q['challenge'][0] == data and elapsed >= 0.8
            else:
                assert q['result'][0] == hashlib.sha256(data.encode()).hexdigest()
                assert elapsed >= 0.08
            proofs[a] = proofs.get(a, 0) + 1
            self.reply(302, '', {
                'Location': q['redir'][0],
                'Set-Cookie': f'auth_{a}={generation}; Path=/{a}',
            })
            return
        a = u.path.strip('/')
        if a not in algorithms:
            self.reply(404, 'unknown')
            return
        cookie = self.headers.get('Cookie', '')
        if f'auth_{a}={generation}' in cookie:
            self.reply(200, 'protected-' + a)
            return
        if f'auth_{a}=' in cookie:
            self.reply(403, 'Expired session')
            return
        issued[a] = time.monotonic()
        diff = 1
        random = data
        if a in ['sha256', 'argon2id', 'hashx']:
            v = next((v for v in vectors if v['algorithm'] == a))
            diff = v['difficulty']
            random = v['data']
        challenge = {
            'rules': {'algorithm': a, 'difficulty': diff},
            'challenge': {'id': a, 'randomData': random},
        }
        self.reply(
            403,
            '<script id="anubis_challenge">' + json.dumps(challenge) + '</script>',
            {'Set-Cookie': f'verification={a}; Path=/'},
        )


class NoRedirect(urllib.request.HTTPRedirectHandler):

    def redirect_request(self, *args, **kwargs):
        return None


opener = urllib.request.build_opener(NoRedirect)


class Proxy(Handler):

    def do_GET(self):
        assert self.path.startswith(target_url + '/'), self.path
        headers = {
            k: v for k, v in self.headers.items()
            if k.lower() not in ['host', 'connection']
        }
        assert not headers.get('Authorization')
        try:
            r = opener.open(urllib.request.Request(self.path, headers=headers), timeout=10)
        except urllib.error.HTTPError as e:
            r = e
        body = r.read()
        self.send_response(r.status)
        for k, v in r.headers.items():
            if k.lower() not in ['content-length', 'transfer-encoding', 'connection']:
                self.send_header(k, v)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
