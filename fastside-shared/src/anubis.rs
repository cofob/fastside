//! Anubis proof-of-work support for service probes.
use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};

use crate::captcha::{
    CaptchaSolver, Challenge, Solution, SolverError, answer_url, parse_challenge, script,
};
#[cfg(test)]
use crate::captcha::{Data, Rules};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, Response, StatusCode};
use ring::digest::{Context as HashContext, SHA256};
#[cfg(test)]
use serde::Deserialize;

const SOLVE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct LocalSolver;

#[async_trait::async_trait]
impl CaptchaSolver for LocalSolver {
    async fn solve(&self, challenge: Challenge) -> Result<Solution, SolverError> {
        challenge.validate()?;
        let parameters = solution_parameters(challenge)
            .await
            .map_err(|error| SolverError::Failed(error.to_string()))?;
        Ok(Solution { parameters })
    }
}

pub struct ProbeResponse {
    pub status: StatusCode,
    pub body: String,
    pub solved: bool,
    pub reused: bool,
    pub algorithm: Option<String>,
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 15) as usize] as char);
    }
    output
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    ensure!(
        value.len().is_multiple_of(2) && value.len() <= 8192,
        "Invalid Anubis challenge size"
    );
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |b: u8| (b as char).to_digit(16).context("Invalid Anubis hex data");
            Ok((digit(pair[0])? * 16 + digit(pair[1])?) as u8)
        })
        .collect()
}

fn leading_zero_bits(hash: &[u8], bits: usize) -> bool {
    bits <= hash.len() * 8
        && hash[..bits / 8].iter().all(|b| *b == 0)
        && (bits.is_multiple_of(8) || hash[bits / 8] >> (8 - bits % 8) == 0)
}

fn solve(challenge: &Challenge, deadline: Instant) -> Result<(u64, String)> {
    let algorithm = challenge.rules.algorithm.as_str();
    ensure!(
        matches!(algorithm, "fast" | "slow" | "sha256" | "argon2id" | "hashx"),
        "Unsupported Anubis algorithm: {algorithm}"
    );
    let legacy = matches!(algorithm, "fast" | "slow");
    let bits = if legacy {
        challenge
            .rules
            .difficulty
            .checked_mul(4)
            .context("Invalid difficulty")?
    } else {
        challenge.rules.difficulty
    };
    ensure!(bits <= 256, "Invalid Anubis difficulty");
    let data = if legacy {
        challenge.challenge.random_data.as_bytes().to_vec()
    } else {
        decode_hex(&challenge.challenge.random_data)?
    };
    let little_endian = data.last().copied().unwrap_or(0) >= 128;
    let hashx = if algorithm == "hashx" {
        let mut seed = data.clone();
        let mut counter = 0u32;
        Some(loop {
            ensure!(Instant::now() < deadline, "Anubis solver timed out");
            if let Ok(hash) = hashx::HashX::new(&seed) {
                break hash;
            }
            seed.clone_from(&data);
            seed.extend_from_slice(&counter.to_le_bytes());
            counter = counter
                .checked_add(1)
                .context("Anubis HashX seed range exhausted")?;
        })
    } else {
        None
    };
    let mut base = HashContext::new(&SHA256);
    base.update(&data);
    let argon2 = argon2::Argon2::default();
    let mut memory = if algorithm == "argon2id" {
        vec![argon2::Block::default(); argon2.params().block_count()]
    } else {
        Vec::new()
    };
    let mut decimal = [b'0'; 10];
    let mut decimal_start = 9;
    let mut bytes = [0; 32];
    let use_argon2 = algorithm == "argon2id";
    for nonce in 0..=u32::MAX {
        // Argon2 has a high cost per nonce. Check its deadline on every iteration.
        if use_argon2 || nonce % 1024 == 0 {
            ensure!(Instant::now() < deadline, "Anubis solver timed out");
        }
        if let Some(hashx) = &hashx {
            bytes = hashx.hash_to_bytes(nonce as u64);
        } else if use_argon2 {
            let salt = if little_endian {
                (nonce as u64).to_le_bytes()
            } else {
                (nonce as u64).to_be_bytes()
            };
            argon2
                .hash_password_into_with_memory(&data, &salt, &mut bytes, &mut memory)
                .map_err(|error| anyhow::anyhow!("Anubis Argon2: {error}"))?;
        } else {
            let mut hash = base.clone();
            if legacy {
                hash.update(&decimal[decimal_start..]);
            } else {
                hash.update(&if little_endian {
                    nonce.to_le_bytes()
                } else {
                    nonce.to_be_bytes()
                });
            }
            bytes.copy_from_slice(hash.finish().as_ref());
        }
        if leading_zero_bits(&bytes, bits) {
            ensure!(Instant::now() < deadline, "Anubis solver timed out");
            return Ok((nonce as u64, hex(&bytes)));
        }
        if legacy {
            // Update the decimal nonce in place without formatting or allocating.
            for index in (0..10).rev() {
                if decimal[index] != b'9' {
                    decimal[index] += 1;
                    decimal_start = decimal_start.min(index);
                    break;
                }
                decimal[index] = b'0';
            }
        }
    }
    bail!("Anubis nonce range exhausted")
}

async fn solution_parameters(challenge: Challenge) -> Result<Vec<(String, String)>> {
    let start = Instant::now();
    match challenge.rules.algorithm.as_str() {
        "metarefresh" | "preact" => {
            let preact = challenge.rules.algorithm == "preact";
            let milliseconds = (challenge.rules.difficulty as u64)
                .checked_mul(if preact { 125 } else { 1000 })
                .and_then(|value| value.checked_add(if preact { 0 } else { 1000 }))
                .context("Invalid Anubis delay")?;
            let delay = Duration::from_millis(milliseconds);
            ensure!(
                delay <= SOLVE_TIMEOUT,
                "Anubis delay exceeds solver time limit"
            );
            tokio::time::sleep(delay).await;
            let (key, value) = if preact {
                (
                    "result",
                    hex(
                        ring::digest::digest(&SHA256, challenge.challenge.random_data.as_bytes())
                            .as_ref(),
                    ),
                )
            } else {
                ("challenge", challenge.challenge.random_data)
            };
            Ok(vec![(key.into(), value)])
        }
        _ => {
            // Bound concurrent memory-heavy work across all probes.
            static WORKERS: LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
                LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(2)));
            let permit = WORKERS.clone().acquire_owned().await?;
            let result = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                solve(&challenge, Instant::now() + SOLVE_TIMEOUT)
            })
            .await?;
            let (nonce, hash) = result?;
            Ok(vec![
                ("response".into(), hash),
                ("nonce".into(), nonce.to_string()),
                (
                    "elapsedTime".into(),
                    start.elapsed().as_millis().to_string(),
                ),
            ])
        }
    }
}

async fn read_body(mut response: Response) -> Result<String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            body.len() + chunk.len() <= MAX_BODY,
            "Probe response is too large"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Solve one challenge with the probe's client, cookies, proxy, and user agent.
/// The caller must use a client with a cookie store.
pub async fn resolve(client: &Client, response: Response) -> Result<ProbeResponse> {
    resolve_with_solver(client, response, &LocalSolver).await
}

pub async fn resolve_with_solver(
    client: &Client,
    response: Response,
    solver: &(dyn CaptchaSolver + Send + Sync),
) -> Result<ProbeResponse> {
    let url = response.url().clone();
    let status = response.status();
    let body = read_body(response).await?;
    let Some(challenge) = parse_challenge(&body)? else {
        return Ok(ProbeResponse {
            status,
            body,
            solved: false,
            reused: false,
            algorithm: None,
        });
    };
    let algorithm = challenge.rules.algorithm.clone();
    let solution = solver.solve(challenge.clone()).await?;
    let endpoint = answer_url(&url, &body, &challenge, solution)?;
    let passed = client
        .get(endpoint.clone())
        .send()
        .await
        .context("Anubis proof submission failed")?;
    ensure!(
        passed.status().is_success()
            || passed.status().is_redirection()
            || passed.url() != &endpoint,
        "Anubis rejected proof: {}",
        passed.status()
    );
    // With redirects enabled, the proof response already contains the target page.
    let response = if passed.url() != &endpoint {
        passed
    } else {
        client
            .get(url)
            .send()
            .await
            .context("Service request after Anubis proof failed")?
    };
    let status = response.status();
    let body = read_body(response).await?;
    ensure!(
        script(&body, "anubis_challenge").is_none(),
        "Anubis challenge remains after solving"
    );
    Ok(ProbeResponse {
        status,
        body,
        solved: true,
        reused: false,
        algorithm: Some(algorithm),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge(difficulty: usize) -> Challenge {
        Challenge {
            rules: Rules {
                algorithm: "fast".into(),
                difficulty,
            },
            challenge: Data {
                id: "test".into(),
                random_data: "test".into(),
            },
        }
    }

    #[test]
    fn proof_matches_sha256_and_zero_nibbles() {
        for difficulty in [0, 1, 2, 3] {
            let (nonce, hash) =
                solve(&challenge(difficulty), Instant::now() + SOLVE_TIMEOUT).unwrap();
            let expected = ring::digest::digest(&SHA256, format!("test{nonce}").as_bytes());
            assert_eq!(
                hash,
                expected
                    .as_ref()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            assert!(hash.starts_with(&"0".repeat(difficulty)));
        }
    }

    #[test]
    fn rejects_invalid_or_expired_work() {
        assert!(solve(&challenge(65), Instant::now() + SOLVE_TIMEOUT).is_err());
        assert!(solve(&challenge(1), Instant::now()).is_err());
        let mut c = challenge(0);
        c.rules.algorithm = "unknown".into();
        assert!(solve(&c, Instant::now() + SOLVE_TIMEOUT).is_err());
    }

    #[tokio::test]
    async fn exchanges_proof_and_cookies_with_redirects_enabled_and_disabled() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        for (follow, algorithm) in [false, true].into_iter().flat_map(|follow| {
            [
                "fast",
                "slow",
                "sha256",
                "argon2id",
                "hashx",
                "metarefresh",
                "preact",
            ]
            .map(|algorithm| (follow, algorithm))
        }) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let target = format!("http://{}/service?q=test", listener.local_addr().unwrap());
            let server_target = target.clone();
            let server = std::thread::spawn(move || {
                for step in 0..3 {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut byte = [0];
                    while !request.ends_with(b"\r\n\r\n") {
                        stream.read_exact(&mut byte).unwrap();
                        request.push(byte[0]);
                    }
                    let request = String::from_utf8(request).unwrap();
                    let (status, headers, body) = match step {
                        0 => (
                            "200 OK",
                            "Set-Cookie: techaro.lol-anubis-cookie-verification=test; Path=/\r\n"
                                .to_string(),
                            format!(
                                r#"<script id="anubis_challenge">{{"rules":{{"algorithm":"{algorithm}","difficulty":0}},"challenge":{{"id":"test","randomData":"74657374"}}}}</script><script id="anubis_base_prefix">"/guard"</script>"#
                            ),
                        ),
                        1 => {
                            assert!(
                                request.contains("techaro.lol-anubis-cookie-verification=test")
                            );
                            let path = request.split_whitespace().nth(1).unwrap();
                            let url = url::Url::parse(&server_target).unwrap().join(path).unwrap();
                            assert_eq!(
                                url.path(),
                                "/guard/.within.website/x/cmd/anubis/api/pass-challenge"
                            );
                            let query: std::collections::HashMap<_, _> =
                                url.query_pairs().into_owned().collect();
                            assert_eq!(query["id"], "test");
                            assert_eq!(query["redir"], server_target);
                            match algorithm {
                                "metarefresh" => assert_eq!(query["challenge"], "74657374"),
                                "preact" => assert_eq!(
                                    query["result"],
                                    hex(ring::digest::digest(&SHA256, b"74657374").as_ref())
                                ),
                                _ => {
                                    assert!(query["elapsedTime"].parse::<u128>().is_ok());
                                    assert_eq!(query["nonce"], "0");
                                    assert_eq!(query["response"].len(), 64);
                                }
                            }
                            (
                                "302 Found",
                                format!(
                                    "Location: {server_target}\r\nSet-Cookie: techaro.lol-anubis-auth=valid; Path=/\r\n"
                                ),
                                String::new(),
                            )
                        }
                        _ => {
                            assert!(request.starts_with("GET /service?q=test "));
                            assert!(request.contains("techaro.lol-anubis-auth=valid"));
                            (
                                if algorithm == "slow" {
                                    "503 Service Unavailable"
                                } else {
                                    "200 OK"
                                },
                                String::new(),
                                "service content".into(),
                            )
                        }
                    };
                    write!(stream, "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
            });
            let policy = if follow {
                reqwest::redirect::Policy::default()
            } else {
                reqwest::redirect::Policy::none()
            };
            let client = Client::builder()
                .no_proxy()
                .cookie_store(true)
                .redirect(policy)
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            let response = client.get(target).send().await.unwrap();
            let response = resolve(&client, response).await.unwrap();
            assert!(response.solved);
            assert_eq!(
                response.status,
                if algorithm == "slow" {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::OK
                }
            );
            assert_eq!(response.body, "service content");
            server.join().unwrap();
        }
    }

    #[test]
    fn matches_upstream_wasm_algorithm_vectors() {
        #[derive(Deserialize)]
        struct Vector {
            algorithm: String,
            data: String,
            difficulty: usize,
            nonce: u64,
            hash: String,
        }
        let vectors: Vec<Vector> =
            serde_json::from_str(include_str!("../tests/fixtures/anubis-vectors.json")).unwrap();
        for vector in vectors {
            let c = Challenge {
                rules: Rules {
                    algorithm: vector.algorithm.clone(),
                    difficulty: vector.difficulty,
                },
                challenge: Data {
                    id: "vector".into(),
                    random_data: vector.data,
                },
            };
            let (nonce, hash) = solve(&c, Instant::now() + Duration::from_secs(60)).unwrap();
            assert_eq!(
                (nonce, hash),
                (vector.nonce, vector.hash),
                "{}",
                vector.algorithm
            );
        }
    }

    #[test]
    #[ignore = "manual release benchmark"]
    fn benchmark_solver() {
        for (algorithm, difficulty, data, repeats) in [
            ("fast", 4, "0123456789abcdef".repeat(8), 30),
            ("sha256", 16, "0123456789abcdef".repeat(8), 30),
            ("hashx", 12, "0123456789abcdef".repeat(8), 10),
            ("argon2id", 2, "0123456789abcdef".repeat(8), 5),
        ] {
            let c = Challenge {
                rules: Rules {
                    algorithm: algorithm.into(),
                    difficulty,
                },
                challenge: Data {
                    id: "benchmark".into(),
                    random_data: data,
                },
            };
            let start = Instant::now();
            let mut nonce = 0;
            for _ in 0..repeats {
                nonce = std::hint::black_box(
                    solve(&c, Instant::now() + Duration::from_secs(60)).unwrap(),
                )
                .0;
            }
            eprintln!(
                "{algorithm}: {:.3} ms/solve, nonce={nonce}",
                start.elapsed().as_secs_f64() * 1000.0 / repeats as f64
            );
        }
    }

    #[test]
    fn validates_bits_and_hex_data() {
        assert!(leading_zero_bits(&[0, 0x7f], 9));
        assert!(!leading_zero_bits(&[0, 0x80], 9));
        assert!(leading_zero_bits(&[0; 32], 256));
        assert!(!leading_zero_bits(&[0; 32], 257));
        assert_eq!(decode_hex("00aAFF").unwrap(), [0, 170, 255]);
        for value in ["0", "zz", "é"] {
            assert!(decode_hex(value).is_err());
        }
    }

    #[tokio::test]
    async fn timed_challenges_wait_and_reject_excessive_delays() {
        for (algorithm, delay) in [("preact", 125), ("metarefresh", 2000)] {
            let mut c = challenge(1);
            c.rules.algorithm = algorithm.into();
            let start = Instant::now();
            solution_parameters(c).await.unwrap();
            assert!(start.elapsed() >= Duration::from_millis(delay));
            let mut c = challenge(usize::MAX);
            c.rules.algorithm = algorithm.into();
            assert!(solution_parameters(c).await.is_err());
        }
    }

    #[test]
    fn reads_json_scripts() {
        assert_eq!(
            script(
                "<script type='application/json' id='anubis_challenge'>\n{}\n</script>",
                "anubis_challenge"
            ),
            Some("\n{}\n")
        );
        assert_eq!(
            script(
                "ordinary page mentioning anubis_challenge",
                "anubis_challenge"
            ),
            None
        );
    }
}
