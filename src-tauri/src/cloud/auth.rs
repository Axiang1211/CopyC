//! Google 登入：桌面 App 的標準流程（PKCE + 本機回呼 http://127.0.0.1:隨機埠）。
//!
//! 只要求三個權限：openid、email（顯示登入的是哪個帳號），以及 drive.appdata
//! （只能讀寫 CopyC 自己在 Drive 裡的隱藏資料夾，看不到使用者的其他檔案）。

use super::CloudError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const SCOPES: &str = "openid email https://www.googleapis.com/auth/drive.appdata";
/// 使用者在瀏覽器裡最多可以花這麼久完成登入。
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// Google Cloud 主控台建立的「電腦版應用程式」用戶端，於建置時由 GitHub Secrets 帶入。
pub const CLIENT_ID: Option<&str> = option_env!("COPYC_GOOGLE_CLIENT_ID");
/// 電腦版用戶端的密鑰不具機密性（Google 文件也這麼說），但部分專案仍要求附上。
const CLIENT_SECRET: Option<&str> = option_env!("COPYC_GOOGLE_CLIENT_SECRET");

#[derive(Debug, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub expires_in: u64,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
}

/// id_token 裡我們需要的欄位。
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub email: String,
    pub exp: u64,
}

/// 解出 id_token 的內容。這個 token 是剛剛透過 TLS 直接從 Google 拿到的，不需要在這裡驗簽；
/// 中繼伺服器那端才會驗證簽章。
pub fn claims(id_token: &str) -> Option<Claims> {
    let payload = id_token.split('.').nth(1)?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

fn random_b64(len: usize) -> String {
    let mut buf = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn client_id() -> Result<&'static str, CloudError> {
    CLIENT_ID.filter(|s| !s.is_empty()).ok_or(CloudError::Unavailable)
}

/// 開瀏覽器讓使用者登入並同意，回傳 token。`page` 是登入完成後瀏覽器分頁顯示的訊息。
pub async fn sign_in(http: &reqwest::Client, page: &str) -> Result<Tokens, CloudError> {
    let client_id = client_id()?;
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|_| CloudError::Network)?;
    let port = listener.local_addr().map_err(|_| CloudError::Network)?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let verifier = random_b64(32);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_b64(16);

    let url = reqwest::Url::parse_with_params(
        AUTH_URL,
        &[
            ("client_id", client_id),
            ("redirect_uri", &redirect),
            ("response_type", "code"),
            ("scope", SCOPES),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ],
    )
    .map_err(|_| CloudError::Network)?;
    crate::platform::open_url(url.as_str());

    let code = tokio::time::timeout(SIGN_IN_TIMEOUT, wait_for_code(&listener, &state, page))
        .await
        .map_err(|_| CloudError::Cancelled)??;

    let mut form = vec![
        ("client_id", client_id),
        ("code", code.as_str()),
        ("code_verifier", verifier.as_str()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect.as_str()),
    ];
    if let Some(secret) = CLIENT_SECRET.filter(|s| !s.is_empty()) {
        form.push(("client_secret", secret));
    }
    token_request(http, &form).await
}

/// 等瀏覽器帶著授權碼回到本機。其他請求（例如 favicon）回 404 繼續等。
async fn wait_for_code(listener: &TcpListener, state: &str, page: &str) -> Result<String, CloudError> {
    loop {
        let (mut sock, _) = listener.accept().await.map_err(|_| CloudError::Network)?;
        let mut buf = vec![0u8; 8192];
        let n = sock.read(&mut buf).await.unwrap_or(0);
        let head = String::from_utf8_lossy(&buf[..n]);
        let target = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("");
        let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else { continue };
        let param = |k: &str| url.query_pairs().find(|(key, _)| key == k).map(|(_, v)| v.into_owned());

        if param("state").as_deref() != Some(state) {
            let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            continue;
        }
        let body = format!(
            "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'>\
             <title>CopyC</title><body style=\"font:16px system-ui;display:grid;place-items:center;height:90vh;\
             margin:0;background:#F6F1E8;color:#2A1C14\"><p>{}</p>",
            html_escape(page)
        );
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = sock.write_all(resp.as_bytes()).await;
        let _ = sock.shutdown().await;
        return match (param("code"), param("error")) {
            (Some(code), _) => Ok(code),
            _ => Err(CloudError::Cancelled),
        };
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// 用 refresh token 換新的 access token（Google 也會一併給新的 id_token）。
pub async fn refresh(http: &reqwest::Client, refresh_token: &str) -> Result<Tokens, CloudError> {
    let client_id = client_id()?;
    let mut form = vec![("client_id", client_id), ("refresh_token", refresh_token), ("grant_type", "refresh_token")];
    if let Some(secret) = CLIENT_SECRET.filter(|s| !s.is_empty()) {
        form.push(("client_secret", secret));
    }
    token_request(http, &form).await
}

async fn token_request(http: &reqwest::Client, form: &[(&str, &str)]) -> Result<Tokens, CloudError> {
    let resp = http.post(TOKEN_URL).form(form).send().await.map_err(|_| CloudError::Network)?;
    let status = resp.status();
    if status.is_success() {
        return resp.json::<Tokens>().await.map_err(|_| CloudError::Network);
    }
    // invalid_grant：使用者在 Google 帳號裡撤銷了授權，或 token 過期，需要重新登入
    let body = resp.text().await.unwrap_or_default();
    if status.as_u16() == 400 && body.contains("invalid_grant") {
        Err(CloudError::SignedOut)
    } else {
        Err(CloudError::Network)
    }
}

/// 登出時撤銷授權，失敗也無妨（本機的 token 會一併刪除）。
pub async fn revoke(http: &reqwest::Client, token: &str) {
    let _ = http.post(REVOKE_URL).form(&[("token", token)]).send().await;
}

/// refresh token 的保存位置：Windows 存在「認證管理員」；其他平台存在只有本人可讀的設定檔
/// （macOS 的鑰匙圈會綁定程式簽章，未付費簽章的版本每次更新都會跳出授權詢問）。
pub mod store {
    use std::path::Path;

    #[cfg(windows)]
    fn entry() -> Option<keyring::Entry> {
        keyring::Entry::new("CopyC", "google-refresh-token").ok()
    }

    #[cfg(windows)]
    pub fn save(_dir: &Path, token: &str) -> bool {
        entry().is_some_and(|e| e.set_password(token).is_ok())
    }

    #[cfg(windows)]
    pub fn load(_dir: &Path) -> Option<String> {
        entry()?.get_password().ok()
    }

    #[cfg(windows)]
    pub fn delete(_dir: &Path) {
        if let Some(e) = entry() {
            let _ = e.delete_credential();
        }
    }

    #[cfg(not(windows))]
    const FILE: &str = "cloud-token";

    #[cfg(not(windows))]
    pub fn save(dir: &Path, token: &str) -> bool {
        use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join(FILE);
        let _ = std::fs::remove_file(&path);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .and_then(|mut f| f.write_all(token.as_bytes()))
            .is_ok()
    }

    #[cfg(not(windows))]
    pub fn load(dir: &Path) -> Option<String> {
        std::fs::read_to_string(dir.join(FILE)).ok().filter(|s| !s.is_empty())
    }

    #[cfg(not(windows))]
    pub fn delete(dir: &Path) {
        let _ = std::fs::remove_file(dir.join(FILE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_id_token_claims() {
        let payload = URL_SAFE_NO_PAD.encode(r#"{"sub":"1234","email":"a@b.com","exp":99,"aud":"x"}"#);
        let c = claims(&format!("eyJhbGciOiJSUzI1NiJ9.{payload}.sig")).unwrap();
        assert_eq!((c.sub.as_str(), c.email.as_str(), c.exp), ("1234", "a@b.com", 99));
        assert!(claims("garbage").is_none());
    }
}
