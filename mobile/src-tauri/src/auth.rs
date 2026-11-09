//! Google 授權（Android 原型）：呼叫 Android 原生的授權畫面（GoogleAuthPlugin.kt）取得 access token，
//! 權限和電腦版一樣只有 openid、email、drive.appdata。access token 大約一小時到期，到期時在背景
//! 再要一次（使用者已同意過就不會跳出畫面）。帳號資訊（email、sub）從 Google 的 tokeninfo 取得。

use crate::cloud::CloudError;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

const TOKENINFO_URL: &str = "https://oauth2.googleapis.com/tokeninfo";
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";

/// Android 端的授權外掛；桌面上開發時沒有。
pub struct GoogleAuth<R: Runtime>(#[allow(dead_code)] pub tauri::plugin::PluginHandle<R>);

pub fn plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("google-auth")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                let handle = api.register_android_plugin("top.dreamsworld.copyc", "GoogleAuthPlugin")?;
                app.manage(GoogleAuth(handle));
            }
            #[cfg(not(target_os = "android"))]
            let _ = (app, api);
            Ok(())
        })
        .build()
}

pub fn available(app: &AppHandle) -> bool {
    app.try_state::<GoogleAuth<tauri::Wry>>().is_some()
}

#[derive(Serialize)]
#[allow(dead_code)]
struct AuthorizeArgs {
    interactive: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct AuthorizeOut {
    access_token: String,
}

/// 取得 access token。`interactive` 為 false 時只在背景換新的，需要使用者同意就回傳 SignedOut。
#[cfg(target_os = "android")]
pub async fn authorize(app: &AppHandle, interactive: bool) -> Result<String, CloudError> {
    let auth = app.try_state::<GoogleAuth<tauri::Wry>>().ok_or(CloudError::Unavailable)?;
    match auth.0.run_mobile_plugin_async::<AuthorizeOut>("authorize", AuthorizeArgs { interactive }).await {
        Ok(out) => Ok(out.access_token),
        Err(e) => {
            let msg = e.to_string();
            Err(if msg.contains("cancelled") {
                CloudError::Cancelled
            } else if msg.contains("consent") {
                CloudError::SignedOut
            } else {
                CloudError::SignInFailed
            })
        }
    }
}

/// 桌面上開發時沒有 Google 授權外掛。
#[cfg(not(target_os = "android"))]
pub async fn authorize(_app: &AppHandle, _interactive: bool) -> Result<String, CloudError> {
    let _ = AuthorizeArgs { interactive: false };
    let _: Option<AuthorizeOut> = None;
    Err(CloudError::Unavailable)
}

/// tokeninfo 的回應：數字欄位是字串。
#[derive(Debug, Deserialize)]
pub struct TokenInfo {
    pub sub: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub expires_in: String,
}

impl TokenInfo {
    pub fn expires_in_secs(&self) -> u64 {
        self.expires_in.parse().unwrap_or(0)
    }
}

pub async fn token_info(http: &reqwest::Client, access_token: &str) -> Result<TokenInfo, CloudError> {
    let resp = http.get(TOKENINFO_URL).query(&[("access_token", access_token)]).send().await?;
    match resp.status().as_u16() {
        200..=299 => resp.json().await.map_err(|_| CloudError::Network),
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
    fn parses_tokeninfo_strings() {
        let info: TokenInfo = serde_json::from_str(
            r#"{"azp":"x.apps.googleusercontent.com","aud":"x.apps.googleusercontent.com","sub":"123","scope":"openid email","exp":"1793900000","expires_in":"3599","email":"a@b.com","email_verified":"true"}"#,
        )
        .unwrap();
        assert_eq!((info.sub.as_str(), info.email.as_str(), info.expires_in_secs()), ("123", "a@b.com", 3599));
    }
}
