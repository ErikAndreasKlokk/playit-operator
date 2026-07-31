//! Provision a **self-managed** playit.gg agent key via the claim flow.
//!
//! ```text
//! cargo run --bin claim            # prints a claim URL, waits for you to accept,
//!                                  # then prints the agent secret key to stdout
//! cargo run --bin claim > key.txt  # capture just the secret (guidance goes to stderr)
//! ```
//!
//! Use the printed secret as `PLAYIT_AGENT_KEY` for the operator, and as the
//! `SECRET_KEY` of a running playit agent (same key) for the data plane. A
//! self-managed agent can create/modify its own tunnels via the V1 API.
//!
//! No credential is needed to run this — the claim endpoints are unauthenticated
//! (that is the point: you are minting a new key).

use std::time::{Duration, Instant};

use serde_json::json;

const API_BASE: &str = "https://api.playit.gg";
/// Overall time to wait for the user to accept before giving up.
const CLAIM_TIMEOUT: Duration = Duration::from_secs(300);
const POLL_INTERVAL: Duration = Duration::from_secs(3);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let http = reqwest::Client::new();

    // Random 8-byte claim code -> 16 hex chars (matches the official plugin).
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("rng failure: {e}"))?;
    let code = hex(&bytes);
    let version = concat!("playit-operator/", env!("CARGO_PKG_VERSION"));

    eprintln!();
    eprintln!("  Claim a self-managed playit agent — open this URL, sign in, and Accept:");
    eprintln!();
    eprintln!("      https://playit.gg/claim/{code}");
    eprintln!();
    eprintln!(
        "  Waiting up to {}s for you to accept…",
        CLAIM_TIMEOUT.as_secs()
    );

    // Poll /claim/setup with the same code; it registers the code and reports
    // status until the user accepts.
    let deadline = Instant::now() + CLAIM_TIMEOUT;
    loop {
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for the claim to be accepted — re-run and try again");
        }
        let data = call(
            &http,
            "/claim/setup",
            &json!({ "code": code, "agent_type": "self-managed", "version": version }),
        )
        .await?;
        let status = data.as_str().unwrap_or_default().to_ascii_lowercase();
        if status.contains("accept") {
            eprintln!("  Accepted — exchanging for the secret key…");
            break;
        } else if status.contains("reject") {
            anyhow::bail!("claim was rejected");
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Exchange the accepted claim for the agent secret key.
    let data = call(&http, "/claim/exchange", &json!({ "code": code })).await?;
    let secret = data
        .get("secret_key")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow::anyhow!("no secret_key in exchange response: {data}"))?;

    eprintln!();
    eprintln!("  ✅ Self-managed agent secret key (below, on stdout). Keep it secret.");
    eprintln!("     Set it as PLAYIT_AGENT_KEY for the operator and SECRET_KEY for a");
    eprintln!("     running playit agent (same key).");
    eprintln!();
    println!("{secret}");
    Ok(())
}

/// POST `body` to `path` (no auth) and return the envelope's `data`, turning a
/// playit error envelope into an error.
async fn call(
    http: &reqwest::Client,
    path: &str,
    body: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let v: serde_json::Value = http
        .post(format!("{API_BASE}{path}"))
        .json(body)
        .send()
        .await?
        .json()
        .await?;
    if v.get("status").and_then(|s| s.as_str()) == Some("success") {
        Ok(v.get("data").cloned().unwrap_or(serde_json::Value::Null))
    } else {
        let kind = v
            .pointer("/data/type")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");
        let msg = v
            .pointer("/data/message")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        anyhow::bail!("playit {path}: {kind} {msg} (raw: {v})")
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
