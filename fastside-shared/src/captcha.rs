//! Portable Anubis challenge protocol. Solvers calculate answers; callers own HTTP sessions.
use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

pub const MAX_CHALLENGE_BYTES: usize = 8192;
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Challenge {
    pub rules: Rules,
    pub challenge: Data,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Rules {
    pub algorithm: String,
    pub difficulty: usize,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Data {
    pub id: String,
    pub random_data: String,
}

#[derive(Debug, Error)]
pub enum SolverError {
    #[error("invalid challenge: {0}")]
    Invalid(String),
    #[error("captcha solver failed: {0}")]
    Failed(String),
}

impl Challenge {
    pub fn validate(&self) -> Result<(), SolverError> {
        let invalid = |message: &str| SolverError::Invalid(message.into());
        if self.challenge.id.is_empty()
            || self.challenge.id.len() > 256
            || self.challenge.random_data.len() > MAX_CHALLENGE_BYTES
        {
            return Err(invalid("invalid ID or data size"));
        }
        let difficulty = self.rules.difficulty;
        let allowed = match self.rules.algorithm.as_str() {
            "fast" | "slow" => difficulty <= 64,
            "sha256" | "argon2id" | "hashx" => {
                let data = self.challenge.random_data.as_bytes();
                difficulty <= 256
                    && data.len().is_multiple_of(2)
                    && data.iter().all(u8::is_ascii_hexdigit)
            }
            "metarefresh" => difficulty <= 9,
            "preact" => difficulty <= 80,
            _ => false,
        };
        if !allowed {
            return Err(invalid("unsupported algorithm, difficulty, or data"));
        }
        Ok(())
    }
}

/// Answer parameters for the Anubis pass-challenge endpoint.
#[derive(Debug, Deserialize, Serialize)]
pub struct Solution {
    pub parameters: Vec<(String, String)>,
}

impl Solution {
    pub fn validate(&self, challenge: &Challenge) -> Result<(), SolverError> {
        let expected: &[&str] = match challenge.rules.algorithm.as_str() {
            "metarefresh" => &["challenge"],
            "preact" => &["result"],
            _ => &["nonce", "response", "elapsedTime"],
        };
        if self.parameters.len() != expected.len()
            || expected.iter().any(|key| {
                self.parameters
                    .iter()
                    .filter(|(name, value)| name == key && value.len() <= MAX_CHALLENGE_BYTES)
                    .count()
                    != 1
            })
        {
            return Err(SolverError::Failed("invalid answer parameters".into()));
        }
        Ok(())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait CaptchaSolver {
    async fn solve(&self, challenge: Challenge) -> Result<Solution, SolverError>;
}

pub fn script<'a>(body: &'a str, id: &str) -> Option<&'a str> {
    static SCRIPTS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)<script\b[^>]*\bid\s*=\s*["']([^"']+)["'][^>]*>(.*?)</script\s*>"#)
            .unwrap()
    });
    SCRIPTS
        .captures_iter(body)
        .find_map(|c| (c.get(1)?.as_str() == id).then(|| c.get(2).unwrap().as_str()))
}

pub fn parse_challenge(body: &str) -> Result<Option<Challenge>, SolverError> {
    script(body, "anubis_challenge")
        .map(|json| {
            let challenge: Challenge = serde_json::from_str(json)
                .map_err(|_| SolverError::Invalid("malformed Anubis challenge".into()))?;
            challenge.validate()?;
            Ok(challenge)
        })
        .transpose()
}

pub fn answer_url(
    url: &Url,
    body: &str,
    challenge: &Challenge,
    solution: Solution,
) -> Result<Url, SolverError> {
    solution.validate(challenge)?;
    let prefix: String = script(body, "anubis_base_prefix")
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| SolverError::Invalid("invalid Anubis base prefix".into()))?
        .unwrap_or_default();
    if !prefix.is_empty() && (!prefix.starts_with('/') || prefix.starts_with("//")) {
        return Err(SolverError::Invalid("invalid Anubis base prefix".into()));
    }
    let mut endpoint = url.clone();
    endpoint.set_path(&format!(
        "{prefix}/.within.website/x/cmd/anubis/api/pass-challenge"
    ));
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    endpoint
        .query_pairs_mut()
        .append_pair("id", &challenge.challenge.id)
        .append_pair("redir", url.as_str())
        .extend_pairs(solution.parameters);
    Ok(endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_answers_that_change_the_redirect_or_challenge_id() {
        let challenge = Challenge {
            rules: Rules {
                algorithm: "fast".into(),
                difficulty: 1,
            },
            challenge: Data {
                id: "test".into(),
                random_data: "test".into(),
            },
        };
        let solution = Solution {
            parameters: vec![
                ("redir".into(), "https://elsewhere.invalid/".into()),
                ("nonce".into(), "0".into()),
                ("response".into(), "0".into()),
            ],
        };
        assert!(solution.validate(&challenge).is_err());
    }
}
