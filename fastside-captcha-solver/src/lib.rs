//! Token-authenticated proof calculation. This service never fetches target URLs.
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::post,
};
use fastside_shared::{
    anubis::LocalSolver,
    captcha::{CaptchaSolver, Challenge},
};
use serde_json::json;
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

struct SolverState {
    token: String,
    capacity: Semaphore,
    solver: Arc<dyn CaptchaSolver + Send + Sync>,
}

pub fn router(token: String) -> anyhow::Result<Router> {
    router_with_solver(token, Arc::new(LocalSolver))
}

fn router_with_solver(
    token: String,
    solver: Arc<dyn CaptchaSolver + Send + Sync>,
) -> anyhow::Result<Router> {
    anyhow::ensure!(
        !token.trim().is_empty(),
        "FASTSIDE_CAPTCHA_SOLVER_TOKEN must not be empty"
    );
    let state = Arc::new(SolverState {
        token,
        capacity: Semaphore::new(2),
        solver,
    });
    Ok(Router::new()
        .route("/v1/solve", post(solve))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .route_layer(middleware::from_fn_with_state(state.clone(), authorize))
        .with_state(state))
}

async fn authorize(
    State(state): State<Arc<SolverState>>,
    request: Request,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !bool::from(token.as_bytes().ct_eq(state.token.as_bytes())) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    next.run(request).await
}

async fn solve(
    State(state): State<Arc<SolverState>>,
    Json(challenge): Json<Challenge>,
) -> Response {
    if let Err(error) = challenge.validate() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error":error.to_string()})),
        )
            .into_response();
    }
    let Ok(_permit) = state.capacity.try_acquire() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("Retry-After", "1")],
            Json(json!({"error":"solver is busy"})),
        )
            .into_response();
    };
    match state.solver.solve(challenge).await {
        Ok(solution) => Json(solution).into_response(),
        Err(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower_service::Service;

    async fn call(token: Option<&str>, body: String) -> Response {
        let mut app = router("test-token".into()).unwrap();
        let mut request = Request::post("/v1/solve").header("Content-Type", "application/json");
        if let Some(token) = token {
            request = request.header(AUTHORIZATION, token);
        }
        app.call(request.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn authentication_is_required_before_body_parsing() {
        for token in [None, Some("Bearer wrong"), Some("test-token")] {
            assert_eq!(
                call(token, "invalid json".into()).await.status(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert!(router(" ".into()).is_err());
    }

    #[tokio::test]
    async fn rejects_excess_concurrent_work() {
        struct Gate {
            started: Arc<Semaphore>,
            finish: Arc<Semaphore>,
        }
        #[async_trait::async_trait]
        impl CaptchaSolver for Gate {
            async fn solve(
                &self,
                _: Challenge,
            ) -> Result<fastside_shared::captcha::Solution, fastside_shared::captcha::SolverError>
            {
                self.started.add_permits(1);
                self.finish.acquire().await.unwrap().forget();
                Ok(fastside_shared::captcha::Solution {
                    parameters: Vec::new(),
                })
            }
        }
        let started = Arc::new(Semaphore::new(0));
        let finish = Arc::new(Semaphore::new(0));
        let app = router_with_solver(
            "test-token".into(),
            Arc::new(Gate {
                started: started.clone(),
                finish: finish.clone(),
            }),
        )
        .unwrap();
        let request = || {
            Request::post("/v1/solve").header("Content-Type", "application/json").header(AUTHORIZATION, "Bearer test-token")
            .body(Body::from(json!({"rules":{"algorithm":"fast","difficulty":0},"challenge":{"id":"test","randomData":"test"}}).to_string())).unwrap()
        };
        let mut one = app.clone();
        let first_request = request();
        let first = tokio::spawn(async move { one.call(first_request).await.unwrap() });
        let mut two = app.clone();
        let second_request = request();
        let second = tokio::spawn(async move { two.call(second_request).await.unwrap() });
        started.acquire_many(2).await.unwrap().forget();
        let response = app.clone().call(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        finish.add_permits(2);
        assert_eq!(first.await.unwrap().status(), StatusCode::OK);
        assert_eq!(second.await.unwrap().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn solves_all_methods_and_validates_requests() {
        for algorithm in [
            "fast",
            "slow",
            "sha256",
            "argon2id",
            "hashx",
            "metarefresh",
            "preact",
        ] {
            let body = json!({"rules":{"algorithm":algorithm,"difficulty":0},"challenge":{"id":"test","randomData":"74657374"}});
            let response = call(Some("Bearer test-token"), body.to_string()).await;
            assert_eq!(response.status(), StatusCode::OK, "{algorithm}");
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let solution: fastside_shared::captcha::Solution =
                serde_json::from_slice(&bytes).unwrap();
            solution
                .validate(&serde_json::from_value(body).unwrap())
                .unwrap();
        }
        let body = json!({"rules":{"algorithm":"unknown","difficulty":0},"challenge":{"id":"test","randomData":"test"}});
        assert_eq!(
            call(Some("Bearer test-token"), body.to_string())
                .await
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            call(Some("Bearer test-token"), " ".repeat(17 * 1024))
                .await
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }
}
