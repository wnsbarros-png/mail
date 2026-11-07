//! Consent-gated cloud attachments. Upload APIs never run during staging.
pub mod auth;
pub mod providers;
use crate::{
    Core,
    error::{CoreError, Result},
    models::SaveDraftArgs,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const DEFAULT_MAIL_LIMIT: u64 = 25 * 1024 * 1024;
pub const MAX_CLOUD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloudProvider {
    GoogleDrive,
    OneDrive,
    Dropbox,
}
impl CloudProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::GoogleDrive => "Google Drive",
            Self::OneDrive => "OneDrive",
            Self::Dropbox => "Dropbox",
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "google_drive" => Ok(Self::GoogleDrive),
            "one_drive" | "onedrive" => Ok(Self::OneDrive),
            "dropbox" => Ok(Self::Dropbox),
            _ => Err(CoreError::Other(
                "Unsupported cloud provider (MEGA and WebDAV are not implemented)".into(),
            )),
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudSettings {
    pub account_limits: HashMap<String, u64>,
    pub auto_link_over_limit: bool,
    pub default_provider: Option<CloudProvider>,
    pub storage_account_id: Option<i64>,
    /// Public app key, not a client secret. Refresh tokens are keyring-only.
    pub dropbox_app_key: String,
}
impl CloudSettings {
    pub fn mail_limit(&self, id: i64) -> u64 {
        self.account_limits
            .get(&id.to_string())
            .copied()
            .unwrap_or(DEFAULT_MAIL_LIMIT)
            .clamp(1, DEFAULT_MAIL_LIMIT)
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct CloudFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub sha256: String,
}
/// The directory is removed on cancel/expiry/success/error. Files are private
/// immutable copies, so the user approves bytes, not a mutable external path.
pub struct StagedFiles {
    pub files: Vec<CloudFile>,
    directory: tempfile::TempDir,
}
impl StagedFiles {
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkOptions {
    pub provider: CloudProvider,
    pub storage_account_id: i64,
    pub expires_at: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct SharedFile {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub provider: CloudProvider,
    pub url: String,
    pub expires_at: Option<String>,
}
#[derive(Clone, Debug)]
pub struct UploadedFile {
    pub id: String,
    pub name: String,
    pub size: u64,
}
pub type Progress = Arc<dyn Fn(u64, u64) + Send + Sync>;
#[async_trait]
pub trait StorageProvider: Send + Sync {
    fn provider(&self) -> CloudProvider;
    async fn upload(&self, file: &CloudFile, progress: Progress) -> Result<UploadedFile>;
    async fn share(&self, file: &UploadedFile, expires_at: Option<&str>) -> Result<SharedFile>;
    /// Deletes only the file created by this upload, never existing user data.
    async fn delete(&self, id: &str) -> Result<()>;
}
fn invalid(message: &str) -> CoreError {
    CoreError::Other(message.into())
}
pub fn validate_expiry(options: &LinkOptions) -> Result<()> {
    if let Some(expiry) = &options.expires_at {
        if options.provider == CloudProvider::GoogleDrive {
            return Err(invalid(
                "Google Drive anyone links do not support expiry. Choose no expiry or another provider.",
            ));
        }
        let date = chrono::DateTime::parse_from_rfc3339(expiry)
            .map_err(|_| invalid("Expiry must be RFC3339"))?;
        if date.timestamp_millis() <= crate::models::now_ms() {
            return Err(invalid("Expiry must be in the future"));
        }
    }
    Ok(())
}
pub async fn verify(file: &CloudFile) -> Result<()> {
    let mut input = tokio::fs::File::open(&file.path).await?;
    if !input.metadata().await?.is_file() {
        return Err(invalid("Cloud attachment is not a regular file"));
    }
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0; 256 * 1024];
    loop {
        let n = input.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > file.size {
            return Err(invalid("Staged attachment changed"));
        }
        hash.update(&buffer[..n]);
    }
    if size != file.size || format!("{:x}", hash.finalize()) != file.sha256 {
        return Err(invalid("Staged attachment changed"));
    }
    Ok(())
}
impl Core {
    pub fn cloud_settings(&self) -> CloudSettings {
        std::fs::read(self.paths.data_dir.join("cloud-links.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    pub async fn set_dropbox_app_key(&self, key: String) -> Result<()> {
        if key.len() > 256 || key.chars().any(|c| !c.is_ascii_alphanumeric()) {
            return Err(invalid("Invalid Dropbox app key"));
        }
        let mut settings = self.cloud_settings();
        settings.dropbox_app_key = key;
        crate::file_io::write_atomic(
            &self.paths.data_dir.join("cloud-links.json"),
            &serde_json::to_vec_pretty(&settings)?,
            "cloud link settings",
        )
        .await?;
        Ok(())
    }
    pub async fn stage_cloud_files(
        &self,
        inputs: &[crate::models::DraftAttachmentIn],
    ) -> Result<StagedFiles> {
        if inputs.is_empty() || inputs.len() > 32 {
            return Err(invalid("Choose between 1 and 32 cloud attachments"));
        }
        let root = self.paths.data_dir.join("cloud-pending");
        tokio::fs::create_dir_all(&root).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).await?;
        }
        let directory = tempfile::Builder::new()
            .prefix("review-")
            .tempdir_in(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .await?;
        }
        let mut staged = StagedFiles {
            files: Vec::new(),
            directory,
        };
        let mut total = 0u64;
        for (i, input) in inputs.iter().enumerate() {
            crate::files::validate_name(&input.filename)
                .map_err(|_| invalid("Unsafe cloud filename"))?;
            let path = Path::new(&input.file_path);
            if !path.is_absolute() {
                return Err(invalid("Cloud file paths must be absolute"));
            }
            if !tokio::fs::metadata(path).await?.is_file() {
                return Err(invalid("Cloud attachments must be regular files"));
            }
            let mut source = tokio::fs::File::open(path).await?;
            let meta = source.metadata().await?;
            if !meta.is_file() || meta.len() == 0 {
                return Err(invalid("Cloud attachments must be non-empty regular files"));
            }
            if total.saturating_add(meta.len()) > MAX_CLOUD_BYTES {
                return Err(invalid("Cloud attachment selection exceeds 2 GiB"));
            }
            let destination = staged.directory.path().join(i.to_string());
            let mut options = tokio::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut out = options.open(&destination).await?;
            let mut hash = Sha256::new();
            let mut size = 0;
            let mut buffer = vec![0; 256 * 1024];
            loop {
                let n = source.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                size += n as u64;
                if total > MAX_CLOUD_BYTES {
                    return Err(invalid("Cloud attachments exceed 2 GiB"));
                }
                hash.update(&buffer[..n]);
                out.write_all(&buffer[..n]).await?;
            }
            out.flush().await?;
            staged.files.push(CloudFile {
                path: destination,
                name: input.filename.clone(),
                size,
                sha256: format!("{:x}", hash.finalize()),
            });
        }
        Ok(staged)
    }
    pub async fn cloud_provider(&self, options: &LinkOptions) -> Result<Arc<dyn StorageProvider>> {
        validate_expiry(options)?;
        let token = self.cloud_access_token(options).await?;
        Ok(Arc::new(providers::HttpStorage::production(
            options.provider,
            token,
        )?))
    }
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
/// Generated links are included in BOTH MIME alternatives, without interpreting
/// provider filenames or the original plain-text body as HTML.
pub fn insert_links(draft: &mut SaveDraftArgs, links: &[SharedFile]) -> Result<()> {
    let mut text = String::from("\n\nArquivos para baixar / Download files:\n");
    let mut html = String::from("<section><h3>Arquivos para baixar / Download files</h3><ul>");
    for link in links {
        let url =
            url::Url::parse(&link.url).map_err(|_| invalid("Provider returned an invalid link"))?;
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err(invalid("Provider returned an unsafe share link"));
        }
        let expiry = link
            .expires_at
            .as_deref()
            .unwrap_or("sem expiração / no expiry");
        text.push_str(&format!(
            "{} — {} bytes — {}\n{}\n{}\n",
            link.name,
            link.size,
            link.provider.label(),
            link.url,
            expiry
        ));
        html.push_str(&format!(
            "<li><a href=\"{}\">{}</a> — {} bytes — {} — {}</li>",
            escape(&link.url),
            escape(&link.name),
            link.size,
            link.provider.label(),
            escape(expiry)
        ));
    }
    text.push_str("Qualquer pessoa com o link pode baixar / Anyone with the link can download.\n");
    html.push_str("</ul><p>Qualquer pessoa com o link pode baixar / Anyone with the link can download.</p></section>");
    let original_html = draft
        .body_html
        .take()
        .unwrap_or_else(|| format!("<pre>{}</pre>", escape(&draft.body_text)));
    draft.body_text.push_str(&text);
    draft.body_html = Some(format!("{original_html}{html}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draft() -> SaveDraftArgs {
        SaveDraftArgs {
            draft_id: None,
            account_id: 1,
            from: None,
            to: vec![],
            cc: vec![],
            bcc: vec![],
            subject: "Synthetic".into(),
            body_text: "<script>plain & text</script>".into(),
            body_html: None,
            mode: "new".into(),
            in_reply_to_message_id: None,
            in_reply_to_header: None,
            attachments: vec![],
        }
    }
    #[test]
    fn plain_and_html_link_blocks_escape_content() {
        let mut draft = draft();
        let link = SharedFile {
            id: "new".into(),
            name: "<report & quote\">.pdf".into(),
            size: 67108864,
            provider: CloudProvider::Dropbox,
            url: "https://example.invalid/file?a=1&b=2".into(),
            expires_at: Some("2099-01-01T00:00:00Z".into()),
        };
        insert_links(&mut draft, &[link]).unwrap();
        assert!(draft.body_text.contains("67108864 bytes — Dropbox"));
        assert!(draft.body_text.contains("2099-01-01"));
        let html = draft.body_html.unwrap();
        assert!(html.contains("&lt;script&gt;plain &amp; text&lt;/script&gt;"));
        assert!(html.contains("&lt;report &amp; quote&quot;&gt;.pdf"));
        assert!(html.contains("?a=1&amp;b=2"));
        assert!(!html.contains("<script>"));
    }
    #[test]
    fn reject_unsafe_links_and_unsupported_expiry() {
        for url in [
            "javascript:alert(1)",
            "http://example.invalid/a",
            "https://user:password@example.invalid/",
        ] {
            let link = SharedFile {
                id: "new".into(),
                name: "test".into(),
                size: 1,
                provider: CloudProvider::OneDrive,
                url: url.into(),
                expires_at: None,
            };
            assert!(insert_links(&mut draft(), &[link]).is_err());
        }
        assert!(
            validate_expiry(&LinkOptions {
                provider: CloudProvider::GoogleDrive,
                storage_account_id: 1,
                expires_at: Some("2099-01-01T00:00:00Z".into())
            })
            .is_err()
        );
        for expiry in ["not-a-date", "2000-01-01T00:00:00Z"] {
            assert!(
                validate_expiry(&LinkOptions {
                    provider: CloudProvider::Dropbox,
                    storage_account_id: 1,
                    expires_at: Some(expiry.into())
                })
                .is_err()
            );
        }
        assert!(CloudProvider::parse("mega").is_err());
        assert!(CloudProvider::parse("webdav").is_err());
    }
    #[tokio::test]
    async fn cloud_staging_is_private_bounded_immutable_and_removed_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let core = Core::start_mail_ui_with_credentials(
            crate::config::Paths::for_tests(dir.path()),
            Arc::new(crate::accounts::credentials::UnavailableCredentialStore),
        )
        .await
        .unwrap();
        let path = dir.path().join("source.txt");
        tokio::fs::write(&path, b"original").await.unwrap();
        let input = crate::models::DraftAttachmentIn {
            file_path: path.to_string_lossy().into_owned(),
            filename: "source.txt".into(),
        };
        let staged = core
            .stage_cloud_files(std::slice::from_ref(&input))
            .await
            .unwrap();
        let copied = staged.files[0].path.clone();
        tokio::fs::write(&path, b"modified").await.unwrap();
        assert_eq!(tokio::fs::read(&copied).await.unwrap(), b"original");
        verify(&staged.files[0]).await.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&copied).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(staged.directory())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        drop(staged);
        assert!(!copied.exists());
        let file = tokio::fs::File::create(&path).await.unwrap();
        file.set_len(MAX_CLOUD_BYTES + 1).await.unwrap();
        drop(file);
        assert!(
            core.stage_cloud_files(std::slice::from_ref(&input))
                .await
                .is_err()
        );
        let mut unsafe_name = input.clone();
        unsafe_name.filename = "../escape.txt".into();
        assert!(core.stage_cloud_files(&[unsafe_name]).await.is_err());
        let mut relative = input;
        relative.file_path = "relative.txt".into();
        assert!(core.stage_cloud_files(&[relative]).await.is_err());
        core.set_dropbox_app_key("public123".into()).await.unwrap();
        assert_eq!(core.cloud_settings().dropbox_app_key, "public123");
        assert!(
            core.set_dropbox_app_key("not a public key".into())
                .await
                .is_err()
        );
    }
}
