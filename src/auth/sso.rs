use anyhow::{anyhow, Context, Result};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::io::{self, Write};
use tokio::net::TcpListener;

const DEFAULT_CLIENT_ID: &str = "YOUR_CLIENT_ID";
const REQUIRED_SCOPES: &[&str] = &[
    "esi-skills.read_skills.v1",
    "esi-skills.read_skillqueue.v1",
    "esi-characters.read_attributes.v1",
];

// ── PKCE helpers ────────────────────────────────────────────────────────

fn generate_code_verifier() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes)
}

async fn compute_code_challenge(verifier: &str) -> Result<String> {
    let hash = Sha256::digest(verifier.as_bytes());
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&hash))
}

fn random_state() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes)
}

// ── SSO flow ────────────────────────────────────────────────────────────

pub async fn run_pkce_flow() -> Result<super::StoredAccountEntry> {
    let client_id = std::env::var("ESI_CLIENT_ID")
        .unwrap_or_else(|_| DEFAULT_CLIENT_ID.to_string());

    if client_id == DEFAULT_CLIENT_ID {
        eprintln!("Warning: using default client ID.");
        eprintln!("Register an app at https://developers.eveonline.com/applications/");
        eprintln!("Then set ESI_CLIENT_ID=<your-client-id>");
    }

    let verifier = generate_code_verifier();
    let challenge = compute_code_challenge(&verifier).await?;

    let listener = TcpListener::bind("127.0.0.1:0").await
        .context("Failed to bind localhost for callback")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let auth_url = build_authorize_url(&client_id, &challenge, &redirect_uri);

    println!("Opening browser for EVE SSO authorization...");
    println!("If it doesn't open automatically, visit:\n{auth_url}");

    let _ = open_browser(&auth_url);
    let auth_code = wait_for_callback(listener, port).await?;
    exchange_code(&auth_code, &verifier, &client_id, &redirect_uri).await
}

// ── URL building ────────────────────────────────────────────────────────

fn build_authorize_url(client_id: &str, challenge: &str, redirect_uri: &str) -> String {
    format!(
        "https://login.eveonline.com/v2/oauth/authorize?\
         response_type=code&\
         client_id={}&\
         redirect_uri={}&\
         state={}&\
         scope={}&\
         code_challenge={}&\
         code_challenge_method=S256",
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(&random_state()),
        urlencoding::encode(&REQUIRED_SCOPES.join(" ")),
        urlencoding::encode(challenge),
    )
}

// ── Browser open (best effort) ─────────────────────────────────────────

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(url).spawn()?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(url).spawn()?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd")
        .args(["/C", "start"])
        .arg(url)
        .spawn()?;
    Ok(())
}

// ── Local callback server ───────────────────────────────────────────────

async fn wait_for_callback(listener: TcpListener, port: u16) -> Result<String> {
    println!("Waiting for authorization at http://127.0.0.1:{port}/callback ...");
    io::stdout().flush().ok();

    let (mut stream, addr) = listener.accept().await?;

    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).await.unwrap_or(0);
    if n == 0 {
        return Err(anyhow!("Empty response from browser"));
    }

    let request_text = String::from_utf8_lossy(&buf[..n]);
    let code = parse_auth_code_from_request(&request_text)?;

    // Send a success page back to the browser.
    let _ = send_success_page(&mut stream, addr).await;

    println!("\nAuthorization received!");
    Ok(code)
}

async fn send_success_page(stream: &mut tokio::net::TcpStream, _addr: std::net::SocketAddr) {
    use tokio::io::AsyncWriteExt;
    let html = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n<html><body><h2>Authorization Successful</h2><p>You may now close this tab and return to eve-remap.</p></body></html>";
    let _ = stream.write_all(html).await;
}

fn parse_auth_code_from_request(request: &str) -> Result<String> {
    let first_line = request.lines().next()
        .ok_or_else(|| anyhow!("No request line"))?;

    let path_start = first_line.find('/').ok_or_else(|| anyhow!("No path in request"))?;
    let query_start = first_line[path_start..].find('?')
        .map(|i| path_start + i + 1)
        .ok_or_else(|| anyhow!("No query string in callback"))?;

    for param in first_line[query_start..].split('&') {
        if let Some((key, value)) = param.split_once('=') {
            if key == "code" && !value.is_empty() {
                return Ok(value.to_string());
            }
        }
    }

    Err(anyhow!(
        "Callback did not contain an authorization code.\n\
         Request: {}",
        first_line.chars().take(200).collect::<String>()
    ))
}

// ── Token exchange ──────────────────────────────────────────────────────

async fn exchange_code(
    auth_code: &str,
    verifier: &str,
    client_id: &str,
    redirect_uri: &str,
) -> Result<super::StoredAccountEntry> {
    use reqwest::Client;

    #[derive(Debug, serde::Deserialize)]
    struct RawTokenResp {
        access_token: String,
        refresh_token: String,
        expires_in: u32,
        owner_character_id: u64,
        character_name: String,
        scope: String,
    }

    let form = [
        ("grant_type", "authorization_code"),
        ("code", auth_code),
        ("code_verifier", verifier),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
    ];

    let client = Client::new();
    let resp = client.post("https://login.eveonline.com/v2/oauth/token")
        .form(&form)
        .send()
        .await
        .context("Failed to connect to EVE SSO token endpoint")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Token exchange failed ({status}): {body}"));
    }

    let raw: RawTokenResp = resp.json().await.context("Invalid token response format")?;

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    Ok(super::StoredAccountEntry {
        character_id: raw.owner_character_id,
        character_name: raw.character_name,
        access_token: raw.access_token,
        refresh_token: raw.refresh_token,
        expires_at: now_secs + raw.expires_in as u64,
        scopes: raw.scope.split_whitespace().map(|s| s.to_string()).collect(),
        created_at: now_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_code_verifier_length() {
        let v = generate_code_verifier();
        assert!(v.len() >= 43 && v.len() <= 128);
        assert!(!v.contains('+') && !v.contains('/'));
    }

    #[tokio::test]
    async fn test_compute_code_challenge_is_valid_base64url() {
        let challenge = compute_code_challenge("testVerifier").await.unwrap();
        assert_eq!(challenge.len(), 43);
        assert!(!challenge.contains('+') && !challenge.contains('/'));
    }

    #[test]
    fn test_parse_auth_code_success() {
        let req = "GET /callback?code=abc123xyz&state=random HTTP/1.1\r\n";
        assert_eq!(parse_auth_code_from_request(req).unwrap(), "abc123xyz");
    }

    #[test]
    fn test_parse_auth_code_missing() {
        let req = "GET /callback?error=access_denied HTTP/1.1\r\n";
        assert!(parse_auth_code_from_request(req).is_err());
    }

    #[test]
    fn test_build_authorize_url_contains_required_params() {
        let url = build_authorize_url("myclient", "chall123", "http://127.0.0.1:8080/callback");
        assert!(url.starts_with("https://login.eveonline.com/v2/oauth/authorize"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=myclient"));
        assert!(url.contains("code_challenge_method=S256"));
    }
}
