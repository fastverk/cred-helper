//! `fastverk-oidc` — the consumer side of the workload-identity broker.
//!
//! Trades a CI-issued OIDC token for a short-lived fastverk token (RFC 8693
//! token exchange against botnoc-web's `/oidc/token`), then exports it as the
//! `FASTVERK_TOKEN_<HOST>` env var the cred-helper's per-host backend reads — so
//! a consumer repo's bazel build authenticates to fastverk services **keylessly**
//! (no stored secret, just `permissions: id-token: write`).
//!
//! Why a separate tool (not the cred-helper): `credresolve` is deliberately
//! dependency-light (prost only) and the hot-path helper does no network —
//! network/refresh lives in fvd locally, and in CI a one-shot exchange tool
//! feeds the existing env backend. This keeps the per-fetch helper tiny.
//!
//! Usage (GitHub Actions):
//! ```yaml
//! permissions: { id-token: write, contents: read }
//! steps:
//!   - run: fastverk-oidc --endpoint https://id.fastverk.com/oidc/token \
//!                        --target-host app.fastverk.com
//!   # -> exports FASTVERK_TOKEN_APP_FASTVERK_COM to $GITHUB_ENV; later bazel
//!   #    fetches to app.fastverk.com get an Authorization: Bearer header.
//! ```

use std::io::Write;

use anyhow::{anyhow, bail, Context, Result};

/// RFC 8693 grant-type + subject-token-type URNs.
const GRANT_TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
const TOKEN_TYPE_JWT: &str = "urn:ietf:params:oauth:token-type:jwt";

struct Args {
    /// The broker's token-exchange endpoint (botnoc-web `/oidc/token`).
    endpoint: String,
    /// The `aud` to request on the CI OIDC token (must match the broker policy).
    audience: String,
    /// The fastverk service host the minted token authorizes; its
    /// `FASTVERK_TOKEN_<HOST>` env var is what gets exported.
    env_name: String,
}

fn main() -> Result<()> {
    let args = parse_args().context("parsing arguments")?;
    let subject = github_oidc_token(&args.audience).context("obtaining the CI OIDC token")?;
    let token = exchange(&args.endpoint, &subject).context("exchanging for a fastverk token")?;
    emit(&args.env_name, &token).context("exporting the fastverk token")?;
    Ok(())
}

fn usage() -> ! {
    eprintln!(
        "usage: fastverk-oidc --endpoint <url> --target-host <host> [--audience <aud>]\n\
         \n\
         --endpoint     the broker's /oidc/token URL\n\
         --target-host  the fastverk host the token authorizes (-> FASTVERK_TOKEN_<HOST>)\n\
         --env-name     override the exported env var name (default: derived from --target-host)\n\
         --audience     OIDC audience to request (default: fastverk)"
    );
    std::process::exit(2);
}

fn parse_args() -> Result<Args> {
    let mut endpoint = None;
    let mut audience = "fastverk".to_string();
    let mut target_host: Option<String> = None;
    let mut env_name: Option<String> = None;

    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut next = || it.next().ok_or_else(|| anyhow!("{flag} needs a value"));
        match flag.as_str() {
            "--endpoint" => endpoint = Some(next()?),
            "--audience" => audience = next()?,
            "--target-host" => target_host = Some(next()?),
            "--env-name" => env_name = Some(next()?),
            "-h" | "--help" => usage(),
            other => bail!("unknown argument: {other}"),
        }
    }

    let endpoint = endpoint.ok_or_else(|| anyhow!("--endpoint is required"))?;
    let env_name = match (env_name, target_host) {
        (Some(n), _) => n,
        (None, Some(h)) => canonical_env_var(&h),
        (None, None) => bail!("one of --target-host or --env-name is required"),
    };
    Ok(Args { endpoint, audience, env_name })
}

/// Fetch the runner's OIDC token for `audience` from the GitHub Actions OIDC
/// provider (the `ACTIONS_ID_TOKEN_REQUEST_*` env vars `id-token: write` sets).
fn github_oidc_token(audience: &str) -> Result<String> {
    let base = std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL").map_err(|_| {
        anyhow!("ACTIONS_ID_TOKEN_REQUEST_URL not set — add `permissions: id-token: write`")
    })?;
    let bearer = std::env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
        .map_err(|_| anyhow!("ACTIONS_ID_TOKEN_REQUEST_TOKEN not set"))?;
    let url = oidc_request_url(&base, audience);
    let body = ureq::get(&url)
        .set("Authorization", &format!("Bearer {bearer}"))
        .call()
        .context("GET the GitHub OIDC token")?
        .into_string()?;
    parse_field(&body, "value").context("GitHub OIDC response had no 'value'")
}

/// Exchange the subject token for a fastverk access token at the broker.
fn exchange(endpoint: &str, subject_token: &str) -> Result<String> {
    let resp = ureq::post(endpoint).send_form(&[
        ("grant_type", GRANT_TOKEN_EXCHANGE),
        ("subject_token", subject_token),
        ("subject_token_type", TOKEN_TYPE_JWT),
    ]);
    let body = match resp {
        Ok(r) => r.into_string()?,
        // ureq surfaces non-2xx as Err(Status); read the body for the reason.
        Err(ureq::Error::Status(code, r)) => {
            let detail = r.into_string().unwrap_or_default();
            bail!("broker returned HTTP {code}: {detail}");
        }
        Err(e) => return Err(e).context("POST the token exchange"),
    };
    parse_field(&body, "access_token").context("exchange response had no 'access_token'")
}

/// Mask + export the token: in Actions, mask it and append to `$GITHUB_ENV`;
/// otherwise print it to stdout for shell capture.
fn emit(env_name: &str, token: &str) -> Result<()> {
    if let Ok(github_env) = std::env::var("GITHUB_ENV") {
        // Mask first so it never appears in logs.
        println!("::add-mask::{token}");
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&github_env)
            .with_context(|| format!("opening GITHUB_ENV ({github_env})"))?;
        writeln!(f, "{env_name}={token}")?;
        eprintln!("fastverk-oidc: exported {env_name}");
    } else {
        // Not in Actions — emit the token for `T=$(fastverk-oidc …)`.
        println!("{token}");
    }
    Ok(())
}

/// Append `audience=<aud>` to the OIDC request URL (which already carries a
/// query string, e.g. `?api-version=…`).
fn oidc_request_url(base: &str, audience: &str) -> String {
    let sep = if base.contains('?') { '&' } else { '?' };
    format!("{base}{sep}audience={}", urlencode(audience))
}

/// Minimal percent-encoding for an audience value (alnum + `-._~` pass; the
/// rest are %XX). Audiences are simple, but be safe.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `FASTVERK_TOKEN_<sanitized host>` — must match credresolve's
/// `canonical_env_var` so the cred-helper's per-host backend picks it up.
fn canonical_env_var(host: &str) -> String {
    let suffix: String = host
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_uppercase() } else { '_' })
        .collect();
    format!("FASTVERK_TOKEN_{suffix}")
}

/// Extract a top-level string field from a JSON object.
fn parse_field(body: &str, key: &str) -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(body).context("parsing JSON response")?;
    v.get(key)
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing string field '{key}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_matches_credresolve() {
        assert_eq!(canonical_env_var("app.fastverk.com"), "FASTVERK_TOKEN_APP_FASTVERK_COM");
        assert_eq!(canonical_env_var("git.example.com"), "FASTVERK_TOKEN_GIT_EXAMPLE_COM");
    }

    #[test]
    fn request_url_appends_audience() {
        assert_eq!(
            oidc_request_url("https://x/y?api-version=2.0", "fastverk"),
            "https://x/y?api-version=2.0&audience=fastverk"
        );
        // No existing query string -> uses '?'.
        assert_eq!(oidc_request_url("https://x/y", "a b"), "https://x/y?audience=a%20b");
    }

    #[test]
    fn parses_fields() {
        assert_eq!(parse_field(r#"{"value":"jwt123"}"#, "value").unwrap(), "jwt123");
        assert_eq!(
            parse_field(r#"{"access_token":"tok","expires_in":900}"#, "access_token").unwrap(),
            "tok"
        );
        assert!(parse_field(r#"{"other":1}"#, "value").is_err());
        assert!(parse_field("not json", "value").is_err());
    }
}
