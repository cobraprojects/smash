use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, anyhow};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE_ENDPOINT: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const RESPONSES_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
const KEYCHAIN_SERVICE: &str = "app.smash.Smash.chatgpt";
const KEYCHAIN_ACCOUNT: &str = "oauth";
const REFRESH_MARGIN_MS: u64 = 5 * 60 * 1000;
const OAUTH_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct StoredAuth {
    pub access: String,
    pub refresh: String,
    pub expires: u64,
    #[serde(default, rename = "accountId")]
    pub account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: Option<u64>,
    id_token: Option<String>,
}

#[derive(Default)]
struct AuthCache {
    initialized: bool,
    auth: Option<StoredAuth>,
}

static AUTH: LazyLock<Mutex<AuthCache>> = LazyLock::new(|| Mutex::new(AuthCache::default()));

pub(crate) struct OAuthAttempt {
    authorize_url: String,
    listener: TcpListener,
    verifier: String,
    state: String,
}

impl OAuthAttempt {
    pub(crate) fn start() -> anyhow::Result<Self> {
        use base64::Engine as _;
        use rand::RngCore as _;
        use sha2::{Digest as _, Sha256};

        let mut verifier_bytes = [0_u8; 32];
        rand::thread_rng().fill_bytes(&mut verifier_bytes);
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier_bytes);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        let mut state_bytes = [0_u8; 32];
        rand::thread_rng().fill_bytes(&mut state_bytes);
        let state = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(state_bytes);

        let mut url = url::Url::parse(AUTHORIZE_ENDPOINT)?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", CLIENT_ID)
            .append_pair("redirect_uri", OAUTH_REDIRECT_URI)
            .append_pair("scope", "openid profile email offline_access")
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true")
            .append_pair("state", &state)
            .append_pair("originator", "smash");

        let listener = TcpListener::bind("127.0.0.1:1455")
            .context("Smash could not start the ChatGPT login callback")?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            authorize_url: url.to_string(),
            listener,
            verifier,
            state,
        })
    }

    pub(crate) fn authorize_url(&self) -> &str {
        &self.authorize_url
    }

    pub(crate) async fn finish(self) -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::from_std(self.listener)?;
        let (mut stream, _) =
            tokio::time::timeout(std::time::Duration::from_secs(300), listener.accept())
                .await
                .context("ChatGPT login timed out")??;
        let mut request = vec![0_u8; 8192];
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::io::AsyncReadExt::read(&mut stream, &mut request),
        )
        .await??;
        let request = String::from_utf8_lossy(&request[..read]);
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .ok_or_else(|| anyhow!("ChatGPT returned an invalid OAuth callback"))?;
        let url = url::Url::parse(&format!("http://localhost{path}"))?;
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
        let code = params
            .get("code")
            .ok_or_else(|| anyhow!("ChatGPT login did not return an authorization code"))?;
        if params.get("state") != Some(&self.state) {
            return Err(anyhow!("ChatGPT OAuth state did not match"));
        }

        let token_response = reqwest::Client::new()
            .post(TOKEN_ENDPOINT)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", OAUTH_REDIRECT_URI),
                ("client_id", CLIENT_ID),
                ("code_verifier", self.verifier.as_str()),
            ])
            .send()
            .await
            .context("could not exchange the ChatGPT authorization code")?;
        if !token_response.status().is_success() {
            return Err(anyhow!("ChatGPT rejected the Smash OAuth token exchange"));
        }
        let tokens: TokenResponse = token_response.json().await?;
        let account_id = extract_account_id(tokens.id_token.as_deref())
            .or_else(|| extract_account_id(Some(&tokens.access_token)));
        let auth = StoredAuth {
            access: tokens.access_token,
            refresh: tokens.refresh_token,
            expires: now_ms() + tokens.expires_in.unwrap_or(3600) * 1000,
            account_id,
        };
        save_auth(&auth)?;

        let response = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Type: text/html; charset=utf-8\r\n",
            "Connection: close\r\n\r\n",
            "<html><body style=\"font-family:system-ui;background:#111;color:#eee;padding:48px\">",
            "<h1>Smash is connected</h1><p>You can close this window.</p></body></html>"
        );
        tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await?;
        Ok(())
    }
}

pub(crate) fn is_connected() -> bool {
    cached_auth().is_some()
}

pub(crate) async fn send_responses(body: &Value) -> anyhow::Result<Vec<Value>> {
    let mut auth = current_auth().await?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .context("failed to create the Smash ChatGPT client")?;

    for attempt in 0..2 {
        let response = client
            .post(RESPONSES_ENDPOINT)
            .bearer_auth(&auth.access)
            .header(
                "ChatGPT-Account-Id",
                auth.account_id.as_deref().unwrap_or(""),
            )
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .header("originator", "smash")
            .header("openai-beta", "responses=experimental")
            .json(body)
            .send()
            .await
            .context("could not connect Smash to ChatGPT")?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
            auth = refresh_auth(&auth).await?;
            continue;
        }

        let status = response.status();
        let text = response
            .text()
            .await
            .context("failed to read the ChatGPT response stream")?;
        if !status.is_success() {
            let detail = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|value| {
                    value
                        .get("detail")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| {
                            value
                                .pointer("/error/message")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                })
                .unwrap_or_else(|| "unknown ChatGPT error".to_owned());
            return Err(anyhow!("ChatGPT returned {status}: {detail}"));
        }
        return parse_response_stream(&text);
    }

    Err(anyhow!("ChatGPT authentication failed"))
}

fn parse_response_stream(stream: &str) -> anyhow::Result<Vec<Value>> {
    let mut text = String::new();
    let mut output = Vec::new();
    let mut response_error = None;

    for line in stream.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        if data == "[DONE]" {
            continue;
        }
        let event: Value =
            serde_json::from_str(data).context("ChatGPT returned a malformed streaming event")?;
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    text.push_str(delta);
                }
            }
            Some("response.output_item.done") => {
                if let Some(item) = event.get("item")
                    && item.get("type").and_then(Value::as_str) == Some("function_call")
                {
                    let arguments = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
                        .unwrap_or_else(|| json!({}));
                    output.push(json!({
                        "type": "tool_use",
                        "id": item.get("call_id").and_then(Value::as_str).unwrap_or_default(),
                        "name": item.get("name").and_then(Value::as_str).unwrap_or_default(),
                        "input": arguments,
                    }));
                }
            }
            Some("error") => {
                response_error = event
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            Some("response.failed") => {
                response_error = event
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            _ => {}
        }
    }

    if let Some(error) = response_error {
        return Err(anyhow!(error));
    }
    if !text.is_empty() {
        output.insert(0, json!({ "type": "text", "text": text }));
    }
    Ok(output)
}

async fn current_auth() -> anyhow::Result<StoredAuth> {
    let auth = cached_auth().ok_or_else(|| {
        anyhow!("ChatGPT is not connected. Open Smash Settings → AI Providers to sign in.")
    })?;
    if auth.expires > now_ms() + REFRESH_MARGIN_MS {
        Ok(auth)
    } else {
        refresh_auth(&auth).await
    }
}

async fn refresh_auth(current: &StoredAuth) -> anyhow::Result<StoredAuth> {
    let response = reqwest::Client::new()
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", current.refresh.as_str()),
        ])
        .send()
        .await
        .context("could not refresh the Smash ChatGPT login")?;
    if !response.status().is_success() {
        clear_auth();
        return Err(anyhow!(
            "ChatGPT login expired; connect it again in Smash Settings"
        ));
    }
    let tokens: TokenResponse = response
        .json()
        .await
        .context("ChatGPT returned invalid OAuth tokens")?;
    let auth = StoredAuth {
        access: tokens.access_token,
        refresh: tokens.refresh_token,
        expires: now_ms() + tokens.expires_in.unwrap_or(3600) * 1000,
        account_id: extract_account_id(tokens.id_token.as_deref())
            .or_else(|| extract_account_id(Some(&current.access)))
            .or_else(|| current.account_id.clone()),
    };
    save_auth(&auth)?;
    Ok(auth)
}

pub(crate) fn save_auth(auth: &StoredAuth) -> anyhow::Result<()> {
    let json = serde_json::to_string(auth)?;
    save_secret(&json)?;
    let mut cache = AUTH.lock();
    cache.initialized = true;
    cache.auth = Some(auth.clone());
    Ok(())
}

pub(crate) fn clear_auth() {
    let mut cache = AUTH.lock();
    cache.initialized = true;
    cache.auth = None;
    drop(cache);
    let _ = remove_secret();
}

fn cached_auth() -> Option<StoredAuth> {
    let mut cache = AUTH.lock();
    if !cache.initialized {
        // Keep the lock while macOS checks Keychain access so concurrent view renders cannot
        // display several authorization dialogs for the same credential.
        cache.auth = load_auth();
        cache.initialized = true;
    }
    cache.auth.clone()
}

fn load_auth() -> Option<StoredAuth> {
    load_secret()
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
}

fn extract_account_id(token: Option<&str>) -> Option<String> {
    use base64::Engine as _;
    let payload = token?.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("chatgpt_account_id")
        .and_then(Value::as_str)
        .or_else(|| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(|auth| auth.get("chatgpt_account_id"))
                .and_then(Value::as_str)
        })
        .map(str::to_owned)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(target_os = "macos")]
fn save_secret(value: &str) -> anyhow::Result<()> {
    use security_framework::os::macos::keychain::SecKeychain;
    SecKeychain::default()?.set_generic_password(
        KEYCHAIN_SERVICE,
        KEYCHAIN_ACCOUNT,
        value.as_bytes(),
    )?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn load_secret() -> anyhow::Result<String> {
    use security_framework::os::macos::keychain::SecKeychain;
    let (password, _) =
        SecKeychain::default()?.find_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)?;
    String::from_utf8(password.as_ref().to_vec()).context("invalid Smash Keychain entry")
}

#[cfg(target_os = "macos")]
fn remove_secret() -> anyhow::Result<()> {
    use security_framework::os::macos::keychain::SecKeychain;
    if let Ok((_, item)) =
        SecKeychain::default()?.find_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
    {
        item.delete();
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn save_secret(_value: &str) -> anyhow::Result<()> {
    Err(anyhow!(
        "Smash secure ChatGPT storage is not implemented on this platform yet"
    ))
}

#[cfg(not(target_os = "macos"))]
fn load_secret() -> anyhow::Result<String> {
    Err(anyhow!("Smash ChatGPT credentials were not found"))
}

#[cfg(not(target_os = "macos"))]
fn remove_secret() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_and_function_calls_from_responses_sse() {
        let stream = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"run_shell_command\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}\n\n",
            "data: [DONE]\n\n",
        );
        let output = parse_response_stream(stream).unwrap();
        assert_eq!(output[0]["text"], "hello");
        assert_eq!(output[1]["id"], "call_1");
        assert_eq!(output[1]["input"]["command"], "pwd");
    }
}
