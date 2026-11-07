use super::*;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DocumentFilters {
    pub tipo: Option<Kind>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub cnpj: Option<String>,
    pub due_after: Option<String>,
    pub due_before: Option<String>,
    pub account_id: Option<i64>,
    pub limit: Option<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentRecord {
    pub message_id: i64,
    pub account_id: i64,
    pub attachment_id: i64,
    pub index: usize,
    pub filename: String,
    pub source_sha256: String,
    pub document: FinancialDocument,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Extraction {
    pub message_id: i64,
    pub documents: Vec<DocumentRecord>,
    pub warnings: Vec<String>,
    pub truncated: bool,
}
pub fn schema(c: &rusqlite::Connection) -> Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS ai_documents(message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,attachment_id INTEGER NOT NULL REFERENCES attachments(id) ON DELETE CASCADE,doc_index INTEGER NOT NULL,parser_version INTEGER NOT NULL,kind TEXT NOT NULL,cnpj TEXT,recipient_cnpj TEXT,issued_on TEXT,due_date TEXT,source_sha256 TEXT NOT NULL,json TEXT NOT NULL,PRIMARY KEY(attachment_id,doc_index));CREATE INDEX IF NOT EXISTS ai_documents_search ON ai_documents(kind,issued_on,due_date);")?;
    Ok(())
}
use crate::{Core, db::repo};
use chrono::{Local, NaiveDate, TimeZone};
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;
static PARSE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
impl DocumentFilters {
    pub fn validate(&self) -> Result<()> {
        for date in [&self.start, &self.end, &self.due_after, &self.due_before]
            .into_iter()
            .flatten()
        {
            if date.len() != 10 || NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
                return Err(error("Dates must be YYYY-MM-DD"));
            }
        }
        if self
            .start
            .as_ref()
            .zip(self.end.as_ref())
            .is_some_and(|(a, b)| a > b)
            || self
                .due_after
                .as_ref()
                .zip(self.due_before.as_ref())
                .is_some_and(|(a, b)| a > b)
            || self.account_id.is_some_and(|a| a <= 0)
            || self.limit.is_some_and(|n| n == 0 || n > 100)
        {
            return Err(error("Invalid document filters (limit 1–100)"));
        }
        if self
            .cnpj
            .as_ref()
            .is_some_and(|s| s.len() != 14 || !s.bytes().all(|c| c.is_ascii_digit()))
        {
            return Err(error("CNPJ filter must contain exactly 14 digits"));
        }
        Ok(())
    }
}
fn date_of(ms: i64) -> NaiveDate {
    let ms = if (-99_999_999_999..=99_999_999_999).contains(&ms) {
        ms.saturating_mul(1000)
    } else {
        ms
    };
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.date_naive())
        .unwrap_or_else(|| Local::now().date_naive())
}
impl Core {
    /// Cached mail only: never fetch a body, execute content, or contact SEFAZ/banks.
    pub async fn extract_documents(&self, message_id: i64) -> Result<Extraction> {
        self.db.write(|c| schema(c)).await?;
        let(message,attachments)=self.db.read(move|c|{let message=repo::messages::detail(c,message_id)?;let mut q=c.prepare("SELECT id,COALESCE(filename,''),size,file_path FROM attachments WHERE message_id=?1 AND is_inline=0 ORDER BY id LIMIT 17")?;let attachments=q.query_map([message_id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<i64>>(2)?,r.get::<_,Option<String>>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;Ok((message,attachments))}).await?;
        let mut result = Extraction {
            message_id,
            truncated: attachments.len() > 16,
            ..Default::default()
        };
        let mut total = 0usize;
        let reference = date_of(message.date);
        let body = message.text_body.unwrap_or_default();
        let mut roots = Vec::new();
        for root in [
            self.paths.attachments_dir(message.account_id),
            self.paths.files_cache_dir(message.account_id),
        ] {
            if let Ok(root) = tokio::fs::canonicalize(root).await {
                roots.push(root);
            }
        }
        for (id, name, size, cached) in attachments.into_iter().take(16) {
            let name = clean(&name);
            let lower = name.to_lowercase();
            if ![".xml", ".pdf", ".txt", ".csv", ".emv"]
                .iter()
                .any(|s| lower.ends_with(s))
            {
                continue;
            }
            let prefix = format!("attachment:{id}");
            if size.is_some_and(|n| n < 0 || n > MAX_INPUT as i64) {
                result.warnings.push(format!("{prefix}:size_limit"));
                continue;
            }
            let Some(path) = cached else {
                result
                    .warnings
                    .push(format!("{prefix}:not_cached_download_in_app_first"));
                continue;
            };
            let path = match tokio::fs::canonicalize(&path).await {
                Ok(p) if roots.iter().any(|r| p.starts_with(r)) => p,
                _ => {
                    result
                        .warnings
                        .push(format!("{prefix}:unsafe_or_unavailable_cache_path"));
                    continue;
                }
            };
            let file = match tokio::fs::File::open(&path).await {
                Ok(f) => f,
                Err(_) => {
                    result.warnings.push(format!("{prefix}:unavailable"));
                    continue;
                }
            };
            let metadata = file.metadata().await?;
            if !metadata.is_file() || metadata.len() > MAX_INPUT as u64 {
                result
                    .warnings
                    .push(format!("{prefix}:size_or_file_type_limit"));
                continue;
            }
            if total.saturating_add(metadata.len() as usize) > 32 * 1024 * 1024 {
                result.truncated = true;
                break;
            }
            let mut bytes = Vec::new();
            file.take((MAX_INPUT + 1) as u64)
                .read_to_end(&mut bytes)
                .await?;
            if bytes.len() > MAX_INPUT {
                result.warnings.push(format!("{prefix}:size_limit"));
                continue;
            }
            total += bytes.len();
            let hash = format!("{:x}", Sha256::digest(&bytes));
            let filename = name.clone();
            let body = body.clone();
            // One bounded PDF/XML parser at a time, off the UI/async executor.
            let guard = PARSE_LOCK.lock().await;
            let parsed = tokio::task::spawn_blocking(move || {
                parse_bytes(&filename, &bytes, reference, &body)
            })
            .await;
            match parsed {
                Ok(Ok(docs)) => {
                    if docs.is_empty() {
                        result
                            .warnings
                            .push(format!("{prefix}:no_supported_document_or_text_layer"));
                    }
                    for (index, document) in docs.into_iter().enumerate() {
                        result.documents.push(DocumentRecord {
                            message_id,
                            account_id: message.account_id,
                            attachment_id: id,
                            index,
                            filename: name.clone(),
                            source_sha256: hash.clone(),
                            document,
                        });
                    }
                }
                _ => result
                    .warnings
                    .push(format!("{prefix}:invalid_unsupported_or_limit_exceeded")),
            }
            if result.documents.len() >= 64 {
                result.documents.truncate(64);
                result.truncated = true;
                break;
            }
        }
        let mut output_bytes = 0usize;
        result.documents.retain(|r| {
            output_bytes = output_bytes
                .saturating_add(serde_json::to_vec(r).map(|v| v.len()).unwrap_or(usize::MAX));
            if output_bytes > 512 * 1024 {
                result.truncated = true;
                false
            } else {
                true
            }
        });
        // Replace stale evidence even when an attachment became unavailable/invalid.
        let records = result.documents.clone();
        self.db.write(move|c|{let tx=c.unchecked_transaction()?;tx.execute("DELETE FROM ai_documents WHERE message_id=?1",[message_id])?;for row in records{let d=&row.document;let issue=d.issued_at.as_deref().and_then(|s|s.get(..10)).filter(|s|NaiveDate::parse_from_str(s,"%Y-%m-%d").is_ok()).map(str::to_owned).unwrap_or_else(||reference.to_string());tx.execute("INSERT INTO ai_documents(message_id,attachment_id,doc_index,parser_version,kind,cnpj,recipient_cnpj,issued_on,due_date,source_sha256,json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![message_id,row.attachment_id,row.index as i64,PARSER_VERSION,d.kind_name(),d.issuer.as_ref().and_then(|p|p.cnpj.as_deref()),d.recipient.as_ref().and_then(|p|p.cnpj.as_deref()),issue,d.due_date,row.source_sha256,serde_json::to_string(&row).map_err(|_|error("Cannot serialize extracted document"))?])?;}tx.commit()?;Ok(())}).await?;
        Ok(result)
    }
    /// List the local index. Run extract_documents for cached messages first; no implicit downloads.
    pub async fn list_documents(&self, filters: DocumentFilters) -> Result<Vec<DocumentRecord>> {
        filters.validate()?;
        self.db.write(|c| schema(c)).await?;
        self.db.read(move|c|{let kind=filters.tipo.map(|k|FinancialDocument::new(k).kind_name());let mut q=c.prepare("SELECT d.json FROM ai_documents d JOIN messages m ON m.id=d.message_id WHERE d.parser_version=?1 AND (?2 IS NULL OR d.kind=?2) AND (?3 IS NULL OR d.issued_on>=?3) AND (?4 IS NULL OR d.issued_on<=?4) AND (?5 IS NULL OR d.cnpj=?5 OR d.recipient_cnpj=?5) AND (?6 IS NULL OR d.due_date>=?6) AND (?7 IS NULL OR d.due_date<=?7) AND (?8 IS NULL OR m.account_id=?8) ORDER BY d.issued_on DESC,d.message_id DESC,d.attachment_id,d.doc_index LIMIT ?9")?;let values=q.query_map(params![PARSER_VERSION,kind,filters.start,filters.end,filters.cnpj,filters.due_after,filters.due_before,filters.account_id,filters.limit.unwrap_or(100)as i64],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;let mut rows=Vec::new();let mut bytes=0usize;for v in values{bytes=bytes.saturating_add(v.len());if bytes>512*1024{break;}
if v.len()>128*1024{continue;}
if let Ok(mut row)=serde_json::from_str::<DocumentRecord>(&v){row.document.refresh_warnings(Local::now().date_naive());rows.push(row);}}Ok(rows)}).await
    }
    pub async fn export_documents_csv(
        &self,
        folder: String,
        filters: DocumentFilters,
    ) -> Result<String> {
        let rows = self.list_documents(filters).await?;
        let root = PathBuf::from(folder);
        if !root.is_absolute()
            || root
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(error(
                "Choose an existing absolute folder without parent traversal",
            ));
        }
        let root = tokio::fs::canonicalize(root).await?;
        if !root.is_dir() {
            return Err(error("CSV destination is not a folder"));
        }
        for protected in [&self.paths.data_dir, &self.paths.cache_dir] {
            if let Ok(p) = tokio::fs::canonicalize(protected).await
                && root.starts_with(p)
            {
                return Err(error(
                    "CSV destination cannot be inside the mail profile/cache",
                ));
            }
        }
        tokio::task::spawn_blocking(move || write_csv(&root, &rows))
            .await
            .map_err(|_| error("CSV worker failed"))?
    }
}
pub fn csv_cell(s: &str) -> String {
    let s:String = s.chars().filter(|c|!c.is_control()&&!matches!(*c,'\u{200b}'..='\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')).take(16*1024).collect();
    let trimmed = s.trim_start();
    let safe = if trimmed.starts_with(['=', '+', '-', '@']) {
        format!("'{s}")
    } else {
        s
    };
    format!("\"{}\"", safe.replace('"', "\"\""))
}
fn write_csv(folder: &Path, rows: &[DocumentRecord]) -> Result<String> {
    use std::io::Write;
    let path = folder.join(format!(
        "mail-documents-{}.csv",
        format_args!("{:032x}", rand::random::<u128>())
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(b"message_id,attachment_id,type,filename,issuer,issuer_cnpj,recipient,recipient_cnpj,number,series,issued_at,amount_cents,due_date,access_key,code,protocol,status,confidence,warnings\r\n")?;
    for row in rows {
        let d = &row.document;
        let fields = vec![
            row.message_id.to_string(),
            row.attachment_id.to_string(),
            d.kind_name().into(),
            row.filename.clone(),
            d.issuer
                .as_ref()
                .and_then(|p| p.name.clone())
                .unwrap_or_default(),
            d.issuer
                .as_ref()
                .and_then(|p| p.cnpj.clone())
                .unwrap_or_default(),
            d.recipient
                .as_ref()
                .and_then(|p| p.name.clone())
                .unwrap_or_default(),
            d.recipient
                .as_ref()
                .and_then(|p| p.cnpj.clone())
                .unwrap_or_default(),
            d.number.clone().unwrap_or_default(),
            d.series.clone().unwrap_or_default(),
            d.issued_at.clone().unwrap_or_default(),
            d.amount_cents.map(|n| n.to_string()).unwrap_or_default(),
            d.due_date.clone().unwrap_or_default(),
            d.access_key.clone().unwrap_or_default(),
            d.code.clone().unwrap_or_default(),
            d.protocol.clone().unwrap_or_default(),
            d.status.clone().unwrap_or_default(),
            d.confidence.clone(),
            d.warnings.join("; "),
        ];
        writeln!(
            file,
            "{}\r",
            fields
                .iter()
                .map(|s| csv_cell(s))
                .collect::<Vec<_>>()
                .join(",")
        )?;
    }
    file.sync_all()?;
    Ok(path.to_string_lossy().into())
}
