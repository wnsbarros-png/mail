//! Bounded attachment inputs and no-overwrite downloads.
use anyhow::{Context, Result, bail};
use flectar_mail_core::models::DraftAttachmentIn;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const MAX_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_FILES: usize = 32;

pub fn mime_type(filename: &str) -> String {
    flectar_mail_core::queue::mime_guess_from_name(filename)
}

pub fn validate_filename(name: &str) -> Result<()> {
    // Reject separators from *both* platforms, ADS, special devices, trailing
    // dots/spaces and control characters even when running on Unix.
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if name.is_empty()
        || name.len() > 240
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control)
        || name.ends_with(['.', ' '])
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        || !matches!(
            Path::new(name).components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        )
    {
        bail!("filename must be a safe single filename, not a path");
    }
    Ok(())
}

pub async fn validate(value: Option<&Value>) -> Result<Vec<DraftAttachmentIn>> {
    validate_limit(value, MAX_BYTES).await
}

pub async fn validate_limit(
    value: Option<&Value>,
    max_bytes: u64,
) -> Result<Vec<DraftAttachmentIn>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let paths = value
        .as_array()
        .context("attachments must be an array of absolute file paths")?;
    if paths.len() > MAX_FILES {
        bail!("at most {MAX_FILES} attachments are allowed");
    }
    let mut total = 0u64;
    let mut out = Vec::new();
    for item in paths {
        let path = Path::new(
            item.as_str()
                .context("each attachment must be a file path")?,
        );
        if !path.is_absolute() {
            bail!("attachment paths must be absolute");
        }
        let metadata = tokio::fs::metadata(path)
            .await
            .context("attachment is missing or inaccessible")?;
        if !metadata.is_file() {
            bail!("attachment is not a regular file");
        }
        total = total
            .checked_add(metadata.len())
            .context("attachment size overflow")?;
        if total > max_bytes {
            bail!("attachments exceed the selected size limit");
        }
        let path = tokio::fs::canonicalize(path).await?;
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("attachment filename is not UTF-8")?
            .to_string();
        validate_filename(&filename)?;
        out.push(DraftAttachmentIn {
            file_path: path
                .to_str()
                .context("attachment path is not UTF-8")?
                .to_string(),
            filename,
        });
    }
    // The core copies these paths into private draft storage, revalidating
    // regular-file status and the byte budget while reading (including races).
    Ok(out)
}

pub async fn save(source: &Path, folder: &Path, filename: &str) -> Result<PathBuf> {
    validate_filename(filename)?;
    if !folder.is_absolute() {
        bail!("download folder must be absolute");
    }
    let folder = tokio::fs::canonicalize(folder)
        .await
        .context("download folder must already exist")?;
    if !folder.is_dir() {
        bail!("download folder must be a directory");
    }
    let file = tokio::fs::File::open(source).await?;
    if !file.metadata().await?.is_file() {
        bail!("attachment is not a regular file");
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > MAX_BYTES {
        bail!("attachment exceeds the 25 MiB limit");
    }
    let path = folder.join(filename);
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    // create_new also refuses existing symlinks; never truncate an existing file.
    let mut output = options
        .open(&path)
        .await
        .context("destination exists or cannot be created")?;
    if let Err(error) = output.write_all(&bytes).await {
        drop(output);
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error.into());
    }
    output.flush().await?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn filenames_are_cross_platform_safe() {
        for bad in [
            "", "..", ".", "../x", "a/b", "a\\b", "C:x", "/tmp/x", "NUL", "con.txt", "LPT1.pdf",
            "x.", "x ", "a\n",
        ] {
            assert!(validate_filename(bad).is_err(), "accepted {bad:?}");
        }
        validate_filename("relatório.pdf").unwrap();
        assert_eq!(mime_type("report.pdf"), "application/pdf");
    }
    #[tokio::test]
    async fn validates_sizes_types_and_saves_without_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        tokio::fs::write(&source, b"hello").await.unwrap();
        assert_eq!(validate(Some(&json!([source]))).await.unwrap().len(), 1);
        assert!(validate(Some(&json!([dir.path()]))).await.is_err());
        assert!(validate(Some(&json!(["relative.txt"]))).await.is_err());
        assert!(
            validate(Some(&json!([dir.path().join("missing")])))
                .await
                .is_err()
        );
        assert!(validate(Some(&json!("not array"))).await.is_err());
        let dest = save(&source, dir.path(), "copy.txt").await.unwrap();
        assert_eq!(tokio::fs::read(&dest).await.unwrap(), b"hello");
        assert!(save(&source, dir.path(), "copy.txt").await.is_err());
        assert!(save(&source, dir.path(), "../escape").await.is_err());
        let file = tokio::fs::File::create(&source).await.unwrap();
        file.set_len(MAX_BYTES + 1).await.unwrap();
        assert!(validate(Some(&json!([source]))).await.is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn download_refuses_symlink_destination() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        tokio::fs::write(&source, b"original").await.unwrap();
        std::os::unix::fs::symlink(&source, dir.path().join("link")).unwrap();
        assert!(save(&source, dir.path(), "link").await.is_err());
        assert_eq!(tokio::fs::read(&source).await.unwrap(), b"original");
    }
}
