# Instance reputation

Instance reputation is optional and disabled by default. When enabled, the main
instance list shows the permanent upvote and downvote totals for each instance.
Each row has `[+]` and `[-]` buttons. History redirect pages also show these
buttons for the last visited instance when the user returns with browser Back.

Reputation uses the canonical instance base URL as its identity. One URL has one
total across all services. Fastside accepts votes only for exact URLs in the
current services source.

## Weighted selection

The `Weighted` selector uses this weight:

```text
(upvotes + 1) / (downvotes + 1)
```

The default lower bound is `0.1`, and the default upper bound is `10.0`. A new
instance has weight `1.0`. One upvote gives weight `2.0`. One downvote gives
weight `0.5`.

Fastside first filters by health, tags, and preferred instances. It then reads
all needed reputation values in one storage operation. If the read fails, it
uses random selection so redirects continue to work.

## Storage

The native server uses SQLite by default. SQLite uses WAL mode, pooled
connections, and a five-second busy timeout. You can select Redis instead:

```yaml
storage:
  backend: Redis
  redis:
    url: redis://<REDIS_HOST>:6379/
    key_prefix: <KEY_PREFIX>
```

Redis is not available in the Cloudflare Worker. On Cloudflare, one Durable
Object serializes vote updates. It publishes a complete reputation snapshot to
Workers KV at most once per configured publish interval. Redirect selection can
temporarily read older totals from KV, but accepted vote increments are kept in
the Durable Object.

On Cloudflare, store the CAPTCHA secret in the
`FASTSIDE_CAPTCHA_SECRET` Worker secret. This binding overrides
`reputation.captcha.secret`, so the secret does not need to be present in
`FASTSIDE_CONFIG`:

```shell
wrangler secret put FASTSIDE_CAPTCHA_SECRET
```

## CAPTCHA

CAPTCHA protection is optional. With CAPTCHA disabled, a vote button submits the
vote directly. With CAPTCHA enabled, the button opens a confirmation page that
contains one configured widget.

Fastside treats `widget_html` as trusted operator configuration. It is inserted
as raw HTML and can load a self-hosted script or a provider-hosted script. Do not
put user-controlled data in this field.

Verification fails closed. Network errors, timeouts, invalid JSON, and an
unsuccessful result reject the vote. The verification URL, secret values,
custom header values, and static request values are redacted from configuration
debug output.

### Self-hosted Cap

[Cap Standalone](https://github.com/tiagozip/cap) is the recommended
privacy-focused option. Run a Cap Standalone instance, create a site in its
dashboard, and keep the site secret private. The public endpoint and site key go
in the widget. Fastside sends the resulting `cap-token` to that site's
`/siteverify` endpoint.

```yaml
reputation:
  enabled: true
  captcha:
    enabled: true
    widget_html: |
      <script src="<CAP_WIDGET_SCRIPT_URL>"></script>
      <cap-widget data-cap-api-endpoint="<CAP_PUBLIC_ENDPOINT>/<CAP_SITE_KEY>/"></cap-widget>
    token_field: cap-token
    verify_url: <CAP_PUBLIC_ENDPOINT>/<CAP_SITE_KEY>/siteverify
    secret: <CAP_SITE_SECRET>
    encoding: Json
    secret_field: secret
    response_field: response
    success_json_pointer: /success
```

The widget must add a form field with the name in `token_field`. The standard
Cap widget adds `cap-token` when it is inside the Fastside form.

### Google reCAPTCHA

```yaml
reputation:
  enabled: true
  captcha:
    enabled: true
    widget_html: |
      <script src="<RECAPTCHA_WIDGET_SCRIPT_URL>" async defer></script>
      <div class="g-recaptcha" data-sitekey="<RECAPTCHA_SITE_KEY>"></div>
    token_field: g-recaptcha-response
    verify_url: <RECAPTCHA_VERIFY_URL>
    secret: <RECAPTCHA_SECRET>
    encoding: Form
    secret_field: secret
    response_field: response
    success_json_pointer: /success
```

### Cloudflare Turnstile

```yaml
reputation:
  enabled: true
  captcha:
    enabled: true
    widget_html: |
      <script src="<TURNSTILE_WIDGET_SCRIPT_URL>" async defer></script>
      <div class="cf-turnstile" data-sitekey="<TURNSTILE_SITE_KEY>"></div>
    token_field: cf-turnstile-response
    verify_url: <TURNSTILE_VERIFY_URL>
    secret: <TURNSTILE_SECRET>
    encoding: Form
    secret_field: secret
    response_field: response
    success_json_pointer: /success
```

### hCaptcha

```yaml
reputation:
  enabled: true
  captcha:
    enabled: true
    widget_html: |
      <script src="<HCAPTCHA_WIDGET_SCRIPT_URL>" async defer></script>
      <div class="h-captcha" data-sitekey="<HCAPTCHA_SITE_KEY>"></div>
    token_field: h-captcha-response
    verify_url: <HCAPTCHA_VERIFY_URL>
    secret: <HCAPTCHA_SECRET>
    encoding: Form
    secret_field: secret
    response_field: response
    success_json_pointer: /success
```

You can also configure static request fields, custom headers, a timeout, and a
different JSON pointer for the success Boolean. Header values are useful for
providers that authenticate with an HTTP header. The timeout must be greater
than zero.

## Native IP controls

The native server can apply two independent controls:

- A general limit across all vote actions. Its default is 10 votes in 60
  seconds when enabled.
- A one-vote rule that permits one upvote or downvote for one instance in the
  configured window. Its default window is 30 minutes when enabled.

Fastside stores raw `IpAddr` values only in process memory. It does not hash,
persist, or log them. The cleanup task keeps each entry only while an active
configured window needs it. Windows longer than 30 minutes are valid.
An enabled control must have a window greater than zero.

Cloudflare Workers do not support these process-local controls. Worker startup
fails if either control is enabled.
