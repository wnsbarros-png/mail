//! Official REST APIs. Request errors are intentionally sanitized: upload-session
//! URLs, Authorization headers and response bodies may contain credentials.
use super::*;
use reqwest::{Client, Response, StatusCode};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
const CHUNK: usize = 5 * 1024 * 1024; // Multiple of Google 256 KiB and OneDrive 320 KiB.
pub struct HttpStorage {
    kind: CloudProvider,
    client: Client,
    token: String,
    api: String,
    content: String,
    testing: bool,
}
impl HttpStorage {
    pub fn production(kind: CloudProvider, token: String) -> Result<Self> {
        let (api, content) = match kind {
            CloudProvider::GoogleDrive => (
                "https://www.googleapis.com/drive/v3",
                "https://www.googleapis.com/upload/drive/v3",
            ),
            CloudProvider::OneDrive => ("https://graph.microsoft.com/v1.0", ""),
            CloudProvider::Dropbox => (
                "https://api.dropboxapi.com/2",
                "https://content.dropboxapi.com/2",
            ),
        };
        Ok(Self {
            kind,
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .map_err(|_| invalid("Could not initialize cloud transport"))?,
            token,
            api: api.into(),
            content: content.into(),
            testing: false,
        })
    }
    fn session_url(&self, raw: &str) -> Result<String> {
        let u = url::Url::parse(raw).map_err(|_| invalid("Invalid upload session"))?;
        if self.testing && u.scheme() == "http" && u.host_str() == Some("127.0.0.1") {
            return Ok(raw.into());
        }
        let host = u.host_str().unwrap_or("");
        let allowed = match self.kind {
            CloudProvider::GoogleDrive => host == "www.googleapis.com",
            CloudProvider::OneDrive => [
                ".up.1drv.com",
                ".sharepoint.com",
                ".storage.live.com",
                ".onedrive.com",
                ".microsoftpersonalcontent.com",
            ]
            .iter()
            .any(|s| host.ends_with(s)),
            CloudProvider::Dropbox => false,
        };
        if u.scheme() != "https" || !allowed || !u.username().is_empty() || u.password().is_some() {
            return Err(invalid("Untrusted upload-session origin"));
        }
        Ok(raw.into())
    }
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Response> {
        request.send().await.map_err(|_|invalid("Cloud connection failed; no email was queued. Review cloud storage for partial uploads before retrying."))
    }
    async fn value(&self, response: Response) -> Result<Value> {
        if !response.status().is_success() {
            return Err(invalid(&format!(
                "{} refused the request (HTTP {}). No email was queued.",
                self.kind.label(),
                response.status().as_u16()
            )));
        }
        let bytes = crate::http_body::bytes(response, 1024 * 1024, "cloud response")
            .await
            .map_err(|_| invalid("Could not read bounded cloud response"))?;
        serde_json::from_slice(&bytes).map_err(|_| invalid("Invalid cloud response"))
    }
    async fn dropbox_content(&self, route: &str, args: Value, bytes: Vec<u8>) -> Result<Response> {
        // ASCII-only header JSON also supports non-ASCII filenames.
        let json = serde_json::to_string(&args)?;
        let mut header = String::new();
        for c in json.chars() {
            if c.is_ascii() {
                header.push(c);
            } else {
                for unit in c.encode_utf16(&mut [0; 2]) {
                    header.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }

        self.send(
            self.client
                .post(format!("{}{route}", self.content))
                .bearer_auth(&self.token)
                .header("Dropbox-API-Arg", header)
                .header("Content-Type", "application/octet-stream")
                .body(bytes),
        )
        .await
    }
}
fn string(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| invalid("Cloud response omitted a required field"))
}
fn expiry_matches(requested: Option<&str>, confirmed: Option<&str>) -> bool {
    match requested {
        None => true,
        Some(requested) => match (
            chrono::DateTime::parse_from_rfc3339(requested),
            confirmed.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()),
        ) {
            (Ok(a), Some(b)) => a == b,
            _ => false,
        },
    }
}
fn segment(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
#[async_trait]
impl StorageProvider for HttpStorage {
    fn provider(&self) -> CloudProvider {
        self.kind
    }
    async fn upload(&self, file: &CloudFile, progress: Progress) -> Result<UploadedFile> {
        verify(file).await?;
        let prefix = format!("mail-{:032x}-", rand::random::<u128>());
        let mut end = file.name.len().min(255 - prefix.len());
        while !file.name.is_char_boundary(end) {
            end -= 1;
        }
        let unique = format!("{prefix}{}", &file.name[..end]);
        let mut input = tokio::fs::File::open(&file.path).await?;
        let session = match self.kind {
            CloudProvider::GoogleDrive => {
                let response = self
                    .send(
                        self.client
                            .post(format!("{}/files?uploadType=resumable", self.content))
                            .bearer_auth(&self.token)
                            .header("X-Upload-Content-Length", file.size)
                            .header(
                                "X-Upload-Content-Type",
                                crate::queue::mime_guess_from_name(&file.name),
                            )
                            .json(&json!({"name":unique})),
                    )
                    .await?;
                if !response.status().is_success() {
                    return Err(invalid("Google Drive refused to start the upload"));
                }
                self.session_url(
                    response
                        .headers()
                        .get("Location")
                        .and_then(|h| h.to_str().ok())
                        .ok_or_else(|| invalid("Missing Drive upload session"))?,
                )?
            }
            CloudProvider::OneDrive => {
                let value=self.value(self.send(self.client.post(format!("{}/me/drive/root:/{}:/createUploadSession",self.api,segment(&unique))).bearer_auth(&self.token).json(&json!({"item":{"@microsoft.graph.conflictBehavior":"fail","name":unique}}))).await?).await?;
                self.session_url(&string(&value, "uploadUrl")?)?
            }
            CloudProvider::Dropbox => {
                let response = self
                    .dropbox_content(
                        "/files/upload_session/start",
                        json!({"close":false}),
                        Vec::new(),
                    )
                    .await?;
                string(&self.value(response).await?, "session_id")?
            }
        };
        let mut offset = 0u64;
        let mut result = None;
        while offset < file.size {
            let count = (file.size - offset).min(CHUNK as u64) as usize;
            let mut bytes = vec![0; count];
            input.read_exact(&mut bytes).await?;
            let end = offset + count as u64;
            let last = end == file.size;
            let response = match self.kind {
                CloudProvider::GoogleDrive => {
                    self.send(
                        self.client
                            .put(&session)
                            .bearer_auth(&self.token)
                            .header(
                                "Content-Range",
                                format!("bytes {}-{}/{}", offset, end - 1, file.size),
                            )
                            .body(bytes),
                    )
                    .await?
                }
                // Upload URLs are preauthenticated; NEVER send the Graph bearer.
                CloudProvider::OneDrive => {
                    self.send(
                        self.client
                            .put(&session)
                            .header(
                                "Content-Range",
                                format!("bytes {}-{}/{}", offset, end - 1, file.size),
                            )
                            .body(bytes),
                    )
                    .await?
                }
                CloudProvider::Dropbox => {
                    let (route, args) = if last {
                        (
                            "/files/upload_session/finish",
                            json!({"cursor":{"session_id":session,"offset":offset},"commit":{"path":format!("/{unique}"),"mode":"add","autorename":false,"mute":true}}),
                        )
                    } else {
                        (
                            "/files/upload_session/append_v2",
                            json!({"cursor":{"session_id":session,"offset":offset},"close":false}),
                        )
                    };
                    self.dropbox_content(route, args, bytes).await?
                }
            };
            if last {
                let value = self.value(response).await?;
                result = Some(string(&value, "id")?);
            } else {
                match self.kind {
                    CloudProvider::GoogleDrive => {
                        if response.status() != StatusCode::PERMANENT_REDIRECT {
                            return Err(invalid("Drive did not acknowledge the upload chunk"));
                        }
                        let expected = format!("bytes=0-{}", end - 1);
                        if response
                            .headers()
                            .get("Range")
                            .and_then(|h| h.to_str().ok())
                            != Some(expected.as_str())
                        {
                            return Err(invalid("Drive upload offset mismatch"));
                        }
                    }
                    CloudProvider::OneDrive => {
                        let value = self.value(response).await?;
                        let next = format!("{end}-");
                        if value
                            .pointer("/nextExpectedRanges/0")
                            .and_then(Value::as_str)
                            != Some(next.as_str())
                        {
                            return Err(invalid("OneDrive upload offset mismatch"));
                        }
                    }
                    CloudProvider::Dropbox => {
                        if !response.status().is_success() {
                            return Err(invalid("Dropbox refused the upload chunk"));
                        }
                    }
                }
            }
            offset = end;
            progress(offset, file.size);
        }
        Ok(UploadedFile {
            id: result.ok_or_else(|| invalid("Upload did not complete"))?,
            name: file.name.clone(),
            size: file.size,
        })
    }
    async fn share(&self, file: &UploadedFile, expires_at: Option<&str>) -> Result<SharedFile> {
        validate_expiry(&LinkOptions {
            provider: self.kind,
            storage_account_id: 0,
            expires_at: expires_at.map(String::from),
        })?;
        let (url, confirmed_expiry) = match self.kind {
            CloudProvider::GoogleDrive => {
                self.value(self.send(self.client.post(format!("{}/files/{}/permissions",self.api,segment(&file.id))).bearer_auth(&self.token).json(&json!({"type":"anyone","role":"reader","allowFileDiscovery":false}))).await?).await?;
                (
                    format!("https://drive.google.com/file/d/{}/view", segment(&file.id)),
                    None,
                )
            }
            CloudProvider::OneDrive => {
                let mut body = json!({"type":"view","scope":"anonymous"});
                if let Some(expiry) = expires_at {
                    body["expirationDateTime"] = json!(expiry);
                }
                let value = self
                    .value(
                        self.send(
                            self.client
                                .post(format!(
                                    "{}/me/drive/items/{}/createLink",
                                    self.api,
                                    segment(&file.id)
                                ))
                                .bearer_auth(&self.token)
                                .json(&body),
                        )
                        .await?,
                    )
                    .await?;
                if value.pointer("/link/scope").and_then(Value::as_str) != Some("anonymous")
                    || value.pointer("/link/type").and_then(Value::as_str) != Some("view")
                {
                    return Err(invalid(
                        "OneDrive did not grant the approved anonymous view permission (tenant policy may forbid it)",
                    ));
                }
                let expiry = value
                    .get("expirationDateTime")
                    .and_then(Value::as_str)
                    .map(String::from);
                if !expiry_matches(expires_at, expiry.as_deref()) {
                    return Err(invalid("OneDrive did not confirm the requested expiry"));
                }
                (
                    value
                        .pointer("/link/webUrl")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid("OneDrive omitted share URL"))?
                        .into(),
                    expiry,
                )
            }
            CloudProvider::Dropbox => {
                let mut settings = json!({"audience":"public","access":"viewer"});
                if let Some(expiry) = expires_at {
                    settings["expires"] = json!(expiry);
                }
                let value = self
                    .value(
                        self.send(
                            self.client
                                .post(format!(
                                    "{}/sharing/create_shared_link_with_settings",
                                    self.api
                                ))
                                .bearer_auth(&self.token)
                                .json(&json!({"path":file.id,"settings":settings})),
                        )
                        .await?,
                    )
                    .await?;
                if value
                    .pointer("/link_permissions/resolved_visibility/.tag")
                    .and_then(Value::as_str)
                    != Some("public")
                {
                    return Err(invalid("Dropbox did not grant public link access"));
                }
                if value
                    .pointer("/link_permissions/link_access_level/.tag")
                    .and_then(Value::as_str)
                    .is_some_and(|access| access != "viewer")
                {
                    return Err(invalid("Dropbox did not grant view-only link access"));
                }
                let expiry = value
                    .get("expires")
                    .and_then(Value::as_str)
                    .map(String::from);
                if !expiry_matches(expires_at, expiry.as_deref()) {
                    return Err(invalid(
                        "Dropbox did not confirm the requested expiry (paid plan may be required)",
                    ));
                }
                (string(&value, "url")?, expiry)
            }
        };
        let parsed = url::Url::parse(&url).map_err(|_| invalid("Invalid shared link"))?;
        if parsed.scheme() != "https"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(invalid("Unsafe share URL"));
        }
        Ok(SharedFile {
            id: file.id.clone(),
            name: file.name.clone(),
            size: file.size,
            provider: self.kind,
            url,
            expires_at: confirmed_expiry,
        })
    }
    async fn delete(&self, id: &str) -> Result<()> {
        let request = match self.kind {
            CloudProvider::GoogleDrive => {
                self.client
                    .delete(format!("{}/files/{}", self.api, segment(id)))
            }
            CloudProvider::OneDrive => {
                self.client
                    .delete(format!("{}/me/drive/items/{}", self.api, segment(id)))
            }
            CloudProvider::Dropbox => self
                .client
                .post(format!("{}/files/delete_v2", self.api))
                .json(&json!({"path":id})),
        };
        let response = self.send(request.bearer_auth(&self.token)).await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(invalid(
                "Cloud cleanup failed; remove the uploaded file in the provider's website",
            ))
        }
    }
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
