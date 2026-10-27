//! Google Drive 的 App 專用資料夾（appDataFolder）。使用者在雲端硬碟裡看不到這個資料夾，
//! CopyC 也只能存取這個資料夾。裡面放的都是加密過的檔案（金鑰檔除外）。

use super::CloudError;
use serde::Deserialize;

const FILES: &str = "https://www.googleapis.com/drive/v3/files";
const UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3/files";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub id: String,
    pub name: String,
    /// RFC 3339（例如 2026-10-26T17:14:38.123Z），同格式可以直接比字串大小
    pub modified_time: String,
    pub created_time: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct List {
    #[serde(default)]
    files: Vec<File>,
    next_page_token: Option<String>,
}

pub struct Drive<'a> {
    pub http: &'a reqwest::Client,
    pub token: &'a str,
}

fn check(resp: reqwest::Response) -> Result<reqwest::Response, CloudError> {
    match resp.status().as_u16() {
        200..=299 => Ok(resp),
        401 => Err(CloudError::SignedOut),
        403 | 429 => Err(CloudError::RateLimited),
        _ => Err(CloudError::Network),
    }
}

impl Drive<'_> {
    /// 列出資料夾裡的所有檔案。
    pub async fn list(&self) -> Result<Vec<File>, CloudError> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut req = self.http.get(FILES).bearer_auth(self.token).query(&[
                ("spaces", "appDataFolder"),
                ("fields", "nextPageToken,files(id,name,modifiedTime,createdTime)"),
                ("pageSize", "1000"),
            ]);
            if let Some(p) = &page {
                req = req.query(&[("pageToken", p)]);
            }
            let list: List = check(req.send().await.map_err(|_| CloudError::Network)?)?
                .json()
                .await
                .map_err(|_| CloudError::Network)?;
            out.extend(list.files);
            match list.next_page_token {
                Some(p) => page = Some(p),
                None => return Ok(out),
            }
        }
    }

    pub async fn download(&self, id: &str) -> Result<Vec<u8>, CloudError> {
        let resp = self
            .http
            .get(format!("{FILES}/{id}"))
            .bearer_auth(self.token)
            .query(&[("alt", "media")])
            .send()
            .await
            .map_err(|_| CloudError::Network)?;
        Ok(check(resp)?.bytes().await.map_err(|_| CloudError::Network)?.to_vec())
    }

    /// 在 appDataFolder 建立新檔，回傳 id。
    pub async fn create(&self, name: &str, data: Vec<u8>) -> Result<String, CloudError> {
        let boundary = "copyc-boundary-7d1f3a";
        let meta = serde_json::json!({ "name": name, "parents": ["appDataFolder"] });
        let mut body = format!(
            "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n\
             --{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(&data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let resp = self
            .http
            .post(UPLOAD)
            .bearer_auth(self.token)
            .query(&[("uploadType", "multipart"), ("fields", "id")])
            .header("Content-Type", format!("multipart/related; boundary={boundary}"))
            .body(body)
            .send()
            .await
            .map_err(|_| CloudError::Network)?;
        #[derive(Deserialize)]
        struct Created {
            id: String,
        }
        let created: Created = check(resp)?.json().await.map_err(|_| CloudError::Network)?;
        Ok(created.id)
    }

    /// 覆寫既有檔案的內容。
    pub async fn update(&self, id: &str, data: Vec<u8>) -> Result<(), CloudError> {
        let resp = self
            .http
            .patch(format!("{UPLOAD}/{id}"))
            .bearer_auth(self.token)
            .query(&[("uploadType", "media"), ("fields", "id")])
            .header("Content-Type", "application/octet-stream")
            .body(data)
            .send()
            .await
            .map_err(|_| CloudError::Network)?;
        check(resp).map(drop)
    }

    pub async fn delete(&self, id: &str) -> Result<(), CloudError> {
        let resp = self
            .http
            .delete(format!("{FILES}/{id}"))
            .bearer_auth(self.token)
            .send()
            .await
            .map_err(|_| CloudError::Network)?;
        // 已經不存在也算成功
        if resp.status().as_u16() == 404 {
            return Ok(());
        }
        check(resp).map(drop)
    }
}
