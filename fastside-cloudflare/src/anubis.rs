//! Workers keep the HTTP session while a remote host calculates the proof.
use std::{cell::RefCell, collections::VecDeque, sync::Arc, time::Duration};

use async_trait::async_trait;
use cookie_store::CookieStore;
use fastside::crawler::{CrawledInstanceStatus, CrawlerError, InstanceRequest, InstanceResponse};
use fastside_shared::{
    captcha::{
        CaptchaSolver, Challenge, MAX_RESPONSE_BYTES, Solution, SolverError, answer_url,
        parse_challenge,
    },
    config::Proxy,
    request_headers::REQUEST_HEADERS,
};
use futures::{TryStreamExt, future::Either, pin_mut};
use tokio::sync::Mutex;
use url::Url;
use worker::{
    AbortController, Date, Delay, Env, Fetch, Headers, Method, Request, RequestInit,
    RequestRedirect,
};

const SOLVER_URL: &str = "FASTSIDE_CAPTCHA_SOLVER_URL";
const SOLVER_TOKEN: &str = "FASTSIDE_CAPTCHA_SOLVER_TOKEN";

pub struct RemoteSolver {
    url: Url,
    token: String,
}

impl RemoteSolver {
    pub fn from_env(env: &Env) -> worker::Result<Option<Self>> {
        let Some(value) = env.var(SOLVER_URL).ok() else {
            return Ok(None);
        };
        let url = Url::parse(&value.to_string())
            .map_err(|error| worker::Error::RustError(error.to_string()))?;
        if url.scheme() != "https"
            && !(url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
        {
            return Err(worker::Error::RustError(
                "captcha solver URL must use HTTPS (HTTP is allowed on loopback for tests)".into(),
            ));
        }
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(worker::Error::RustError(
                "captcha solver URL must not contain credentials, a query, or a fragment".into(),
            ));
        }
        let token = env.secret(SOLVER_TOKEN)?.to_string();
        if token.trim().is_empty() {
            return Err(worker::Error::RustError(
                "captcha solver token must not be empty".into(),
            ));
        }
        Ok(Some(Self { url, token }))
    }
}

#[async_trait(?Send)]
impl CaptchaSolver for RemoteSolver {
    async fn solve(&self, challenge: Challenge) -> Result<Solution, SolverError> {
        challenge.validate()?;
        let headers = Headers::new();
        headers
            .set("Content-Type", "application/json")
            .map_err(solver_error)?;
        headers
            .set("Authorization", &format!("Bearer {}", self.token))
            .map_err(solver_error)?;
        let body = serde_json::to_string(&challenge).map_err(solver_error)?;
        // Manual redirects keep the API token on the configured solver endpoint.
        let response = fetch(
            self.url.clone(),
            headers,
            Some(body),
            Duration::from_secs(20),
            16 * 1024,
        )
        .await
        .map_err(|_| SolverError::Failed("remote request failed or timed out".into()))?;
        if response.status != 200 {
            return Err(SolverError::Failed(format!(
                "remote solver returned HTTP {}",
                response.status
            )));
        }
        let solution: Solution = serde_json::from_str(&response.body)
            .map_err(|_| SolverError::Failed("remote solver returned invalid JSON".into()))?;
        solution.validate(&challenge)?;
        Ok(solution)
    }
}

fn solver_error(error: impl std::fmt::Display) -> SolverError {
    SolverError::Failed(error.to_string())
}
pub enum ProbeError {
    Request(String),
    TimedOut,
    Redirect,
    Body(String),
}

impl From<CrawlerError> for ProbeError {
    fn from(error: CrawlerError) -> Self {
        Self::Request(error.to_string())
    }
}

fn request_error(error: impl std::fmt::Display) -> ProbeError {
    ProbeError::Request(error.to_string())
}

pub struct WireResponse {
    pub url: Url,
    pub status: u16,
    pub cookies: Vec<String>,
    pub location: Option<String>,
    pub body: String,
}

struct AbortOnDrop(Option<AbortController>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(controller) = self.0.take() {
            controller.abort();
        }
    }
}

async fn fetch(
    url: Url,
    headers: Headers,
    body: Option<String>,
    timeout: Duration,
    limit: usize,
) -> Result<WireResponse, ProbeError> {
    let mut init = RequestInit::new();
    init.with_headers(headers)
        .with_redirect(RequestRedirect::Manual);
    if let Some(body) = body {
        init.with_method(Method::Post).with_body(Some(body.into()));
    }
    let request = Request::new_with_init(url.as_str(), &init).map_err(request_error)?;
    let mut controller = AbortOnDrop(Some(AbortController::default()));
    let signal = controller.0.as_ref().unwrap().signal();
    let work = async {
        let mut response = Fetch::Request(request)
            .send_with_signal(&signal)
            .await
            .map_err(request_error)?;
        let status = response.status_code();
        let cookies = response
            .headers()
            .get_all("set-cookie")
            .map_err(request_error)?;
        let location = response.headers().get("location").map_err(request_error)?;
        let bytes = if let Ok(mut stream) = response.stream() {
            let mut bytes = Vec::new();
            while let Some(chunk) = stream
                .try_next()
                .await
                .map_err(|e| ProbeError::Body(e.to_string()))?
            {
                if bytes.len() + chunk.len() > limit {
                    return Err(ProbeError::Body("response is too large".into()));
                }
                bytes.extend_from_slice(&chunk);
            }
            bytes
        } else {
            let bytes = response
                .bytes()
                .await
                .map_err(|e| ProbeError::Body(e.to_string()))?;
            if bytes.len() > limit {
                return Err(ProbeError::Body("response is too large".into()));
            }
            bytes
        };
        Ok(WireResponse {
            url,
            status,
            cookies,
            location,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        })
    };
    let delay = Delay::from(timeout);
    pin_mut!(work, delay);
    let result = match futures::future::select(work, delay).await {
        Either::Left((result, _)) => result,
        Either::Right(((), _)) => Err(ProbeError::TimedOut),
    };
    // A fully read response can return its connection to the runtime pool.
    // Abort only failed or cancelled requests, not a completed connection.
    if result.is_ok() {
        controller.0.take();
    }
    result
}

#[derive(Default)]
struct Session {
    cookies: CookieStore,
    authenticated: bool,
}

type Sessions = VecDeque<(String, u64, Arc<Mutex<Session>>)>;
thread_local! { static SESSIONS: RefCell<Sessions> = const { RefCell::new(VecDeque::new()) }; }

fn session(url: &Url, proxy: Option<&Proxy>, follow: bool) -> Arc<Mutex<Session>> {
    // Do not log this key: proxy configuration can contain credentials.
    let key = serde_json::to_string(&(url.origin().ascii_serialization(), proxy, follow)).unwrap();
    let now = Date::now().as_millis();
    SESSIONS.with_borrow_mut(|cache| {
        cache.retain(|(_, used, _)| now.saturating_sub(*used) < 30 * 60 * 1000);
        if let Some(index) = cache.iter().position(|(existing, _, _)| existing == &key) {
            let (_, _, session) = cache.remove(index).unwrap();
            cache.push_back((key, now, session.clone()));
            return session;
        }
        if cache.len() >= 256 {
            let index = cache
                .iter()
                .position(|(_, _, value)| value.try_lock().is_ok_and(|s| !s.authenticated))
                .unwrap_or(0);
            cache.remove(index);
        }
        let session = Arc::new(Mutex::new(Session::default()));
        cache.push_back((key, now, session.clone()));
        session
    })
}

async fn get(
    state: &mut Session,
    proxy: Option<&Proxy>,
    mut url: Url,
    follow: bool,
    timeout: Duration,
) -> Result<WireResponse, ProbeError> {
    for hop in 0..=10 {
        let cookie = state
            .cookies
            .get_request_values(&url)
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        let response = if let Some(proxy) = proxy {
            crate::proxy::get(
                proxy,
                url.clone(),
                (!cookie.is_empty()).then_some(cookie.as_str()),
                timeout,
            )
            .await?
        } else {
            let headers = Headers::new();
            for (name, value) in REQUEST_HEADERS {
                headers.set(name, value).map_err(request_error)?;
            }
            if !cookie.is_empty() {
                headers.set("Cookie", &cookie).map_err(request_error)?;
            }
            fetch(url.clone(), headers, None, timeout, MAX_RESPONSE_BYTES).await?
        };
        for cookie in &response.cookies {
            let _ = state.cookies.parse(cookie, &url);
        }
        if !follow
            || !matches!(response.status, 301 | 302 | 303 | 307 | 308)
            || response.location.is_none()
        {
            return Ok(response);
        }
        if hop == 10 {
            return Err(ProbeError::Redirect);
        }
        url = url
            .join(response.location.as_ref().unwrap())
            .map_err(|_| ProbeError::Redirect)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ProbeError::Redirect);
        }
        url.set_fragment(None);
    }
    unreachable!()
}

async fn attempt(
    state: &mut Session,
    solver: Option<&RemoteSolver>,
    proxy: Option<&Proxy>,
    url: Url,
    follow: bool,
    timeout: Duration,
) -> Result<(WireResponse, bool), ProbeError> {
    let response = get(state, proxy, url, follow, timeout).await?;
    let Some(challenge) = parse_challenge(&response.body).map_err(request_error)? else {
        return Ok((response, false));
    };
    let solver = solver.ok_or_else(|| {
        request_error("Anubis challenge requires FASTSIDE_CAPTCHA_SOLVER_URL and its token secret")
    })?;
    let solution = solver
        .solve(challenge.clone())
        .await
        .map_err(request_error)?;
    let endpoint =
        answer_url(&response.url, &response.body, &challenge, solution).map_err(request_error)?;
    let passed = get(state, proxy, endpoint.clone(), follow, timeout).await?;
    if passed.url == endpoint && !(200..400).contains(&passed.status) {
        return Err(request_error(format!(
            "Anubis rejected proof: HTTP {}",
            passed.status
        )));
    }
    let final_response = if passed.url != endpoint {
        passed
    } else {
        get(state, proxy, response.url, follow, timeout).await?
    };
    if parse_challenge(&final_response.body)
        .map_err(request_error)?
        .is_some()
    {
        return Err(request_error("Anubis challenge remains after solving"));
    }
    Ok((final_response, true))
}

async fn request_inner(
    solver: Option<&RemoteSolver>,
    proxy: Option<&Proxy>,
    url: Url,
    follow: bool,
    timeout: Duration,
) -> Result<InstanceRequest, ProbeError> {
    let start = Date::now().as_millis();
    let session = session(&url, proxy, follow);
    let mut state = session.lock().await;
    let result = attempt(&mut state, solver, proxy, url.clone(), follow, timeout).await;
    let retry = state.authenticated
        && match &result {
            Ok((response, solved)) => !solved && matches!(response.status, 401 | 403),
            Err(_) => true,
        };
    let (response, solved) = if retry {
        *state = Session::default();
        attempt(&mut state, solver, proxy, url, follow, timeout).await?
    } else {
        result?
    };
    state.authenticated |= solved;
    Ok(InstanceRequest::Response(InstanceResponse {
        anubis_solved: state.authenticated,
        status_code: response.status,
        body: Some(response.body),
        duration: Duration::from_millis(Date::now().as_millis().saturating_sub(start)),
    }))
}

pub async fn request(
    solver: Option<&RemoteSolver>,
    proxy: Option<&Proxy>,
    url: Url,
    follow: bool,
    timeout: Duration,
) -> Result<InstanceRequest, CrawlerError> {
    let work = request_inner(solver, proxy, url, follow, timeout);
    let deadline = Delay::from(Duration::from_secs(60));
    pin_mut!(work, deadline);
    let result = match futures::future::select(work, deadline).await {
        Either::Left((result, _)) => result,
        Either::Right(((), _)) => Err(ProbeError::TimedOut),
    };
    Ok(match result {
        Ok(response) => response,
        Err(error) => InstanceRequest::Failed(match error {
            ProbeError::TimedOut => CrawledInstanceStatus::TimedOut,
            ProbeError::Redirect => CrawledInstanceStatus::RedirectPolicyError,
            ProbeError::Request(message) => {
                worker::console_error!("Service probe failed: {message}");
                CrawledInstanceStatus::RequestError
            }
            ProbeError::Body(message) => {
                worker::console_error!("Service response failed: {message}");
                CrawledInstanceStatus::BodyError
            }
        }),
    })
}
