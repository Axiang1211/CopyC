//! Google 登入（Android 原型）：裝置代碼流程。App 顯示一組代碼，使用者在 google.com/device 輸入後同意，
//! App 輪詢拿到 token。登入途中 App 被系統凍結也沒關係：回到 App 時繼續輪詢即可。
//!
//! 用的是 Google Cloud 同一個專案裡「電視和輸入受限裝置」類型的用戶端，所以能讀到電腦版放在
//! Drive App 專用資料夾裡的金鑰。權限和電腦版一樣只有 openid、email、drive.appdata。

use crate::cloud::CloudError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;

const DEVICE_URL: &str = "https://oauth2.googleapis.com/device/code";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const SCOPES: &str = "openid email https://www.googleapis.com/auth/drive.appdata";

/// 建置時由 GitHub Secrets 帶入；這類用戶端的密鑰依 Google 文件不具機密性，但輪詢時必須附上。
pub const CLIENT_ID: Option<&str> = option_env!("COPYC_ANDROID_CLIENT_ID");
const CLIENT_SECRET: Option<&str> = option_env!("COPYC_ANDROID_CLIENT_SECRET");

pub fn available() -> bool {
    CLIENT_ID.is_some_and(|s| !s.is_empty()) && CLIENT_SECRET.is_some_and(|s| !s.is_empty())
}

fn client() -> Result<(&'static str, &'static str), CloudError> {
    match (CLIENT_ID, CLIENT_SECRET) {
        (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => Ok((id, secret)),
        _ => Err(CloudError::Unavailable),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

#[derive(Debug, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub expires_in: u64,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub email: String,
    pub exp: u64,
}

/// id_token 的內容（直接從 Google 經 TLS 拿到的，不在這裡驗簽；中繼伺服器會驗）。
pub fn claims(id_token: &str) -> Option<Claims> {
    let payload = id_token.split('.').nth(1)?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

pub async fn start(http: &reqwest::Client) -> Result<DeviceCode, CloudError> {
    let (id, _) = client()?;
    let resp = http.post(DEVICE_URL).form(&[("client_id", id), ("scope", SCOPES)]).send().await?;
    if !resp.status().is_success() {
        return Err(CloudError::Network);
    }
    Ok(resp.json().await?)
}

pub enum Poll {
    Pending,
    SlowDown,
    Done(Tokens),
}

/// 問一次使用者完成了沒。拒絕或代碼過期回傳 Cancelled。
pub async fn poll(http: &reqwest::Client, device_code: &str) -> Result<Poll, CloudError> {
    #[derive(Deserialize)]
    struct Error {
        error: String,
    }
    let (id, secret) = client()?;
    let form = [
        ("client_id", id),
        ("client_secret", secret),
        ("device_code", device_code),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
    ];
    let resp = http.post(TOKEN_URL).form(&form).send().await?;
    if resp.status().is_success() {
        return Ok(Poll::Done(resp.json().await?));
    }
    match resp.json::<Error>().await.map(|e| e.error).unwrap_or_default().as_str() {
        "authorization_pending" => Ok(Poll::Pending),
        "slow_down" => Ok(Poll::SlowDown),
        "access_denied" | "expired_token" => Err(CloudError::Cancelled),
        _ => Err(CloudError::Network),
    }
}

pub async fn refresh(http: &reqwest::Client, refresh_token: &str) -> Result<Tokens, CloudError> {
    let (id, secret) = client()?;
    let form = [
        ("client_id", id),
        ("client_secret", secret),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    let resp = http.post(TOKEN_URL).form(&form).send().await?;
    match resp.status().as_u16() {
        200..=299 => Ok(resp.json().await?),
        400 | 401 => Err(CloudError::SignedOut),
        _ => Err(CloudError::Network),
    }
}

pub async fn revoke(http: &reqwest::Client, token: &str) {
    let _ = http.post(REVOKE_URL).form(&[("token", token)]).send().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_claims_from_id_token() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"123","email":"a@b.com","exp":99}"#);
        let c = claims(&format!("x.{payload}.y")).unwrap();
        assert_eq!((c.sub.as_str(), c.email.as_str(), c.exp), ("123", "a@b.com", 99));
        assert!(claims("not-a-jwt").is_none());
    }

    #[test]
    fn device_code_defaults_interval() {
        let d: DeviceCode = serde_json::from_str(
            r#"{"device_code":"d","user_code":"ABC-DEF","verification_url":"https://www.google.com/device","expires_in":1800}"#,
        )
        .unwrap();
        assert_eq!((d.user_code.as_str(), d.interval), ("ABC-DEF", 5));
    }
}
