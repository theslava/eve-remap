use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// Path to the config directory: ~/.config/eve-remap/
fn config_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("cannot determine config directory")?
        .join("eve-remap");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Path to the accounts file: ~/.config/eve-remap/accounts.json
fn accounts_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("accounts.json"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAccount {
    pub character_id: u64,
    pub character_name: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Expiry as Unix timestamp (seconds)
    pub expires_at: f64,
    /// Client ID used for this authentication
    pub client_id: String,
}

#[allow(dead_code)]
impl StoredAccount {
    pub fn is_expired(&self) -> bool {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        self.expires_at - now < 300.0
    }
}

/// All stored accounts loaded from disk.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AccountsStore {
    pub accounts: Vec<StoredAccount>,
}

impl AccountsStore {
    pub fn load() -> Result<Self> {
        let path = accounts_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path).context("failed to read accounts file")?;
        // Accept either {"accounts":[...]} or bare [...] for backwards compat
        match serde_json::from_str::<Self>(&content) {
            Ok(store) => Ok(store),
            Err(_) => {
                let accounts: Vec<StoredAccount> =
                    serde_json::from_str(&content).context("failed to parse accounts file")?;
                Ok(Self { accounts })
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = accounts_path()?;
        let content = serde_json::to_string_pretty(self).context("failed to serialize accounts")?;
        fs::write(&path, content).context("failed to write accounts file")
    }

    #[allow(dead_code)]
    pub fn get(&self, character_id: u64) -> Option<&StoredAccount> {
        self.accounts
            .iter()
            .find(|a| a.character_id == character_id)
    }

    pub fn remove(&mut self, character_id: u64) -> bool {
        let len_before = self.accounts.len();
        self.accounts.retain(|a| a.character_id != character_id);
        self.accounts.len() < len_before
    }

    pub fn upsert(&mut self, account: StoredAccount) {
        if let Some(existing) = self
            .accounts
            .iter_mut()
            .find(|a| a.character_id == account.character_id)
        {
            *existing = account;
        } else {
            self.accounts.push(account);
        }
    }
}

// ── Local CA / Certificate management ────────────────────────────────────

/// Paths within ~/.config/eve-remap/ for TLS artifacts.
fn tls_paths() -> Result<(PathBuf, PathBuf, PathBuf)> {
    let dir = config_dir()?;
    Ok((
        dir.join("ca.pem"),
        dir.join("server.der"),
        dir.join("server-key.der"),
    ))
}

/// Generate or load the local CA and server certificate for HTTPS callbacks.
fn load_or_generate_tls_config() -> Result<rustls::ServerConfig> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

    let (ca_path, server_der_path, server_key_path) = tls_paths()?;

    // Check if we already have persisted certs
    let (server_cert_der, server_key_der) = if ca_path.exists() && server_der_path.exists() {
        eprintln!("[tls] loading existing certificates");
        let cert = fs::read(&server_der_path).context("failed to read server cert")?;
        let key = fs::read(&server_key_path).context("failed to read server key")?;
        (cert, key)
    } else {
        generate_ca_and_server_certs()?
    };

    // Build rustls config
    let tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(server_cert_der)],
            PrivateKeyDer::try_from(PrivatePkcs8KeyDer::from(server_key_der))
                .context("failed to convert server key for rustls")?,
        )
        .context("failed to build TLS server config")?;

    Ok(tls_config)
}

/// Generate a local CA + server certificate pair and persist DER to disk.
fn generate_ca_and_server_certs() -> Result<(Vec<u8>, Vec<u8>)> {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyUsagePurpose,
    };

    let dir = config_dir()?;
    let ca_pem_path = dir.join("ca.pem");

    // --- CA Certificate (self-signed) ---
    let mut ca_params = CertificateParams::new(vec![]);
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "eve-remap Local CA");
    ca_dn.push(DnType::OrganizationName, "eve-remap");
    ca_params.distinguished_name = ca_dn;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];

    let ca_cert =
        rcgen::Certificate::from_params(ca_params).context("failed to generate CA cert")?;
    let ca_pem = ca_cert
        .serialize_pem()
        .context("failed to serialize CA cert")?;
    fs::write(&ca_pem_path, &ca_pem).context("failed to write CA cert")?;

    // --- Server Certificate (signed by CA) ---
    let mut server_params =
        CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()]);
    let mut server_dn = DistinguishedName::new();
    server_dn.push(DnType::CommonName, "localhost");
    server_params.distinguished_name = server_dn;

    let server_cert =
        rcgen::Certificate::from_params(server_params).context("failed to generate server cert")?;

    // Sign with CA — produces DER
    let server_der = server_cert
        .serialize_der_with_signer(&ca_cert)
        .context("failed to sign server cert with CA")?;

    // Store as binary DER
    fs::write(dir.join("server.der"), &server_der).context("failed to write server cert")?;

    // Store private key as binary DER
    let server_key_der = server_cert.serialize_private_key_der();
    fs::write(dir.join("server-key.der"), &server_key_der).context("failed to write server key")?;

    eprintln!("Generated local TLS certificates at {}.", dir.display());
    eprintln!("\nTo trust this CA in your OS (one-time setup):");
    if cfg!(windows) || std::env::var("WSL_DISTRO_NAME").is_ok() {
        eprintln!(
            "  WSL/Windows: copy {} to Windows and double-click → Install Certificate",
            ca_pem_path.display()
        );
        eprintln!("               → Local Machine → Place all in 'Trusted Root Certification Authorities'");
    } else if cfg!(target_os = "linux") {
        eprintln!(
            "  Linux: sudo cp {} /usr/local/share/ca-certificates/eve-remap-ca.crt",
            ca_pem_path.display()
        );
        eprintln!("         sudo update-ca-trust");
    } else if cfg!(target_os = "macos") {
        eprintln!(
            "  macOS: open {} (Keychain Access) → set Trust to 'Always Trust'",
            ca_pem_path.display()
        );
    }
    eprintln!();

    Ok((server_der, server_key_der))
}

// ── Login flow ───────────────────────────────────────────────────────────

/// Run the full PKCE login flow and return the authenticated account info.
pub async fn login(
    client_id: &str,
    scopes: &[&str],
    port_hint: Option<u16>,
    http_callback: bool,
) -> Result<StoredAccount> {
    let callback_port = find_available_port(port_hint)?;
    let scheme = if http_callback { "http" } else { "https" };
    let callback_url = format!("{scheme}://localhost:{callback_port}/callback");

    // Build rfesi client for public app auth (PKCE, no client_secret)
    let scope_str = scopes.join(" ");
    let mut esi = rfesi::prelude::EsiBuilder::new()
        .user_agent("eve-remap/0.1")
        .client_id(client_id)
        .callback_url(&callback_url)
        .scope(&scope_str)
        .enable_application_authentication(true)
        .build()
        .context("failed to build ESI client")?;

    // Generate authorize URL with PKCE challenge embedded
    let auth_info = esi
        .get_authorize_url()
        .context("failed to generate authorize URL")?;
    let authorize_url = auth_info.authorization_url;
    let state = auth_info.state;
    let pkce_verifier = auth_info.pkce_verifier.ok_or_else(|| {
        anyhow!("PKCE verifier not generated — application authentication may not be enabled")
    })?;

    // Start local listener for the callback
    let (tx, mut rx) = mpsc::channel::<(String, String)>(1);

    let listener = TcpListener::bind(format!("127.0.0.1:{callback_port}"))
        .await
        .with_context(|| format!("failed to bind to port {callback_port}"))?;

    let server_task = if http_callback {
        tokio::spawn(async move {
            listen_for_code(listener, tx, &state).await;
        })
    } else {
        let tls_config = load_or_generate_tls_config()?;
        let tls_acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
        tokio::spawn(async move {
            listen_for_code_tls(listener, tx, &state, tls_acceptor).await;
        })
    };

    println!("Opening browser for ESI authorization...");
    println!("\nAuthorization URL:");
    println!("{authorize_url}\n");
    if !http_callback {
        eprintln!("Note: make sure your OS trusts the eve-remap local CA.\n");
    }
    if let Err(e) = open_browser(&authorize_url) {
        eprintln!("Warning: could not open browser automatically: {e}");
        eprintln!("Please copy the URL above into your browser manually.\n");
    }

    // Wait for the authorization code from the callback handler (with timeout)
    let (auth_code, _) = tokio::time::timeout(std::time::Duration::from_secs(120), rx.recv())
        .await
        .unwrap_or(None)
        .ok_or_else(|| anyhow!("authorization timed out after 120 seconds or was cancelled"))?;

    server_task.abort();

    // Exchange code for tokens via rfesi
    let claims = esi
        .authenticate(&auth_code, Some(pkce_verifier))
        .await
        .context("token exchange failed — invalid authorization code")?;

    let token_claims = claims
        .as_ref()
        .ok_or_else(|| anyhow!("no token claims returned"))?;
    // EVE SSO sub claim format: "eve-online:character:ID:<number>"
    let character_id: u64 = token_claims
        .sub
        .split(':')
        .last()
        .unwrap_or(&token_claims.sub)
        .parse()
        .context("failed to parse character ID from JWT sub claim")?;
    let character_name = token_claims.name.clone();
    let expires_at = token_claims.exp as f64;

    let access_token = esi
        .access_token
        .clone()
        .ok_or_else(|| anyhow!("no access token after authentication"))?;
    let refresh_token = esi.refresh_token.clone().unwrap_or_default();

    Ok(StoredAccount {
        character_id,
        character_name,
        access_token,
        refresh_token,
        expires_at,
        client_id: client_id.to_string(),
    })
}

// ── HTTP listener (plain) ────────────────────────────────────────────────

async fn listen_for_code(
    listener: TcpListener,
    tx: mpsc::Sender<(String, String)>,
    expected_state: &str,
) {
    if let Ok((stream, addr)) = listener.accept().await {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        eprintln!("[listener] connection from {}", addr);
        let (readable, mut writable) = stream.into_split();

        let mut reader = BufReader::new(readable);
        let mut request_line = String::new();
        if let Ok(_) = reader.read_line(&mut request_line).await {
            eprintln!("[listener] request line: {}", request_line.trim());
            let url_start = request_line.find('/');
            if let Some(start) = url_start {
                let uri = &request_line[start..];
                let (code, state) = parse_callback(uri);

                let response = match (&code, &state) {
                    (_, st) if st != expected_state => {
                        eprintln!("[listener] STATE MISMATCH!");
                        eprintln!("  Expected: '{}'", expected_state);
                        eprintln!("  Received: '{}'", st);
                        eprintln!("  Full URI: {}", uri);
                        "HTTP/1.1 403 Forbidden\r\nContent-Length: 19\r\nConnection: close\r\n\r\nState mismatch error"
                    }
                    (Some(c), _) => {
                        eprintln!("[listener] authorization code received");
                        let _ = tx.send((c.clone(), state.clone())).await;
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body><h2>Authorization successful!</h2><p>You may close this window.</p></body></html>"
                    }
                    _ => {
                        eprintln!("[listener] no code in callback URI");
                        "HTTP/1.1 400 Bad Request\r\nContent-Length: 16\r\nConnection: close\r\n\r\nNo code provided"
                    }
                };

                let _ = writable.write_all(response.as_bytes()).await;
            } else {
                eprintln!("[listener] could not parse URL from request line");
            }
        } else {
            eprintln!("[listener] failed to read request");
        }
    } else {
        eprintln!("[listener] failed to accept connection");
    }
}

// ── HTTPS listener (TLS) ────────────────────────────────────────────────

async fn listen_for_code_tls(
    listener: TcpListener,
    tx: mpsc::Sender<(String, String)>,
    expected_state: &str,
    tls_acceptor: tokio_rustls::TlsAcceptor,
) {
    if let Ok((stream, addr)) = listener.accept().await {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        eprintln!("[listener] connection from {}", addr);
        let mut stream = match tls_acceptor.accept(stream).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[listener] TLS handshake failed: {:?}", e);
                return;
            }
        };
        eprintln!("[listener] TLS established");

        let mut reader = BufReader::new(&mut stream);
        let mut request_line = String::new();
        if let Ok(_) = reader.read_line(&mut request_line).await {
            eprintln!("[listener] request line: {}", request_line.trim());
            let url_start = request_line.find('/');
            if let Some(start) = url_start {
                let uri = &request_line[start..];
                let (code, state) = parse_callback(uri);

                let response = match (&code, &state) {
                    (_, st) if st != expected_state => {
                        eprintln!("[listener] STATE MISMATCH!");
                        eprintln!("  Expected: '{}'", expected_state);
                        eprintln!("  Received: '{}'", st);
                        eprintln!("  Full URI: {}", uri);
                        "HTTP/1.1 403 Forbidden\r\nContent-Length: 19\r\nConnection: close\r\n\r\nState mismatch error"
                    }
                    (Some(c), _) => {
                        eprintln!("[listener] authorization code received");
                        let _ = tx.send((c.clone(), state.clone())).await;
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body><h2>Authorization successful!</h2><p>You may close this window.</p></body></html>"
                    }
                    _ => {
                        eprintln!("[listener] no code in callback URI");
                        "HTTP/1.1 400 Bad Request\r\nContent-Length: 16\r\nConnection: close\r\n\r\nNo code provided"
                    }
                };

                let _ = stream.write_all(response.as_bytes()).await;
            } else {
                eprintln!("[listener] could not parse URL from request line");
            }
        } else {
            eprintln!("[listener] failed to read request");
        }
    } else {
        eprintln!("[listener] failed to accept connection");
    }
}

/// Parse authorization code and state from a callback URI like "/callback?code=abc&state=xyz".
fn parse_callback(uri: &str) -> (Option<String>, String) {
    // Strip trailing " HTTP/*\r\n" if present (request line may include it).
    let uri = match uri.find(" HTTP/") {
        Some(pos) => &uri[..pos],
        None => uri,
    };

    let query_start = uri.find('?');
    let mut code = None;
    let mut state = String::new();

    if let Some(start) = query_start {
        let query = &uri[start + 1..];
        for param in query.split('&') {
            if let Some((key, value)) = param.split_once('=') {
                match key {
                    "code" => code = Some(value.to_string()),
                    "state" => state = value.to_string(),
                    _ => {}
                }
            }
        }
    }
    (code, state)
}

/// Try to open a URL in the system browser.
fn open_browser(url: &str) -> std::io::Result<()> {
    // Strategy 1: wslview (WSL → Windows interop, handles URLs properly)
    if let Err(e) = std::process::Command::new("wslview").arg(url).status() {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e);
        }
    } else {
        return Ok(());
    }

    // Strategy 2: xdg-open (Linux desktop environments)
    if let Err(e) = std::process::Command::new("xdg-open").arg(url).status() {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e);
        }
    } else {
        return Ok(());
    }

    // Strategy 3: gnome-open (GNOME fallback)
    if let Ok(status) = std::process::Command::new("gnome-open").arg(url).status() {
        if status.success() {
            return Ok(());
        }
    }

    // Strategy 4: open (macOS)
    if let Ok(status) = std::process::Command::new("open").arg(url).status() {
        if status.success() {
            return Ok(());
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no browser launcher found",
    ))
}

/// Pick the callback port. Defaults to 9090 (EVE SSO requires exact URI match).
fn find_available_port(hint: Option<u16>) -> Result<u16> {
    if let Some(port) = hint {
        Ok(port)
    } else {
        Ok(9090)
    }
}
