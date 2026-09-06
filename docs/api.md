# HTTP API

All endpoints are rooted at the server base URL (default `http://localhost:8080`).  They return JSON unless noted otherwise.

| Path | Method | Description |
|------|--------|-------------|
| `/` | GET | Renders HTML dashboard with crawl status |
| `/favicon.ico` | GET | Returns favicon.ico file |
| `/robots.txt` | GET | Returns robots.txt file |
| `/configure` | GET | Settings page (HTML) |
| `/configure/save?query` | GET | Persists `UserConfig` cookie (query passed as URL query string) |
| `/reputation/vote` | GET | Shows the CAPTCHA confirmation form when CAPTCHA is enabled |
| `/reputation/vote` | POST | Submits an HTML instance vote form |
| `/api/v1/redirect` | POST | Compute redirect target for given URL |
| `/api/v1/make_user_config_string` | POST | Encode `UserConfig` → base64 string (returns JSON-wrapped string) |
| `/api/v1/parse_user_config_string` | POST | Decode base64 → `UserConfig` (expects JSON-wrapped string) |
| `/_/<path>` | GET | History helper that redirects after 1 s |
| `/@cached/<service>/<path>` | GET | Static HTML that lists *all* healthy instances |
| `/<service>/<path>` | GET, POST | Transparent redirect to best instance |
| `<full_url>` | GET, POST | Paste a raw URL to redirect to privacy-friendly mirror |

## Request / Response examples

### POST /api/v1/redirect
```jsonc
// request body
{
  "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
  "config": {
    "select_method": "LowPing",
    "required_tags": ["https"],
    "forbidden_tags": ["onion"]
  }
}
```

```jsonc
// success 200
{
  "url": "https://ytdiff.example/abcd...",
  "is_fallback": false
}
```

API errors use HTTP 404 or 500 and JSON `{ "detail": "..." }`.

## Status codes

* **307 Temporary Redirect** – browser redirect paths.
* **303 See Other** – accepted vote.
* **400 Bad Request** – invalid vote data or return path.
* **403 Forbidden** – invalid CSRF token or failed CAPTCHA.
* **404 Not Found** – disabled reputation or an unknown instance URL.
* **429 Too Many Requests** – an IP vote control rejected the vote.
* **503 Service Unavailable** – the state store could not save the vote.
* **200 OK** – JSON or HTML pages.

The vote route is an HTML form route. Fastside does not expose a public JSON
voting API. The submitted instance URL must exactly match a current configured
instance. `return_to` must be a same-origin relative path.

## CORS

All API routes use the Axum default. They do not add cross-origin headers.
