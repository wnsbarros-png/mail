//! Local, bounded financial-document extraction. Checksums are NOT authenticity checks.
mod codes;
mod pdf;
mod store;
mod text;
mod xml;
use crate::error::{CoreError, Result};
pub use codes::{bank_dv, crc16, decode_boleto, decode_pix, mod10, mod11, validate_access_key};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub use store::{DocumentFilters, DocumentRecord, Extraction, schema};
pub const MAX_INPUT: usize = 16 * 1024 * 1024;
pub const MAX_TEXT: usize = 1024 * 1024;
pub const MAX_DOCUMENTS: usize = 16;
pub const PARSER_VERSION: i64 = 1;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Nfe,
    Nfce,
    Cte,
    Event,
    Boleto,
    Pix,
    Receipt,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Party {
    pub cnpj: Option<String>,
    pub name: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub code: Option<String>,
    pub description: String,
    pub quantity: Option<String>,
    pub unit: Option<String>,
    pub amount_cents: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FinancialDocument {
    pub kind: Kind,
    pub issuer: Option<Party>,
    pub recipient: Option<Party>,
    pub number: Option<String>,
    pub series: Option<String>,
    pub access_key: Option<String>,
    pub issued_at: Option<String>,
    pub amount_cents: Option<i64>,
    pub due_date: Option<String>,
    pub due_candidates: Vec<String>,
    pub items: Vec<Item>,
    pub item_count: usize,
    pub protocol: Option<String>,
    pub status: Option<String>,
    pub code: Option<String>,
    pub fields: BTreeMap<String, String>,
    pub checks: BTreeMap<String, bool>,
    pub confidence: String,
    pub warnings: Vec<String>,
}
impl FinancialDocument {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            issuer: None,
            recipient: None,
            number: None,
            series: None,
            access_key: None,
            issued_at: None,
            amount_cents: None,
            due_date: None,
            due_candidates: vec![],
            items: vec![],
            item_count: 0,
            protocol: None,
            status: None,
            code: None,
            fields: BTreeMap::new(),
            checks: BTreeMap::new(),
            confidence: "structured_unverified".into(),
            warnings: vec!["not_authenticated".into()],
        }
    }
    pub fn warn(&mut self, s: &str) {
        if !self.warnings.iter().any(|w| w == s) {
            self.warnings.push(s.into());
        }
    }
    pub fn refresh_warnings(&mut self, today: chrono::NaiveDate) {
        self.warnings.retain(|w| w != "overdue");
        if self
            .due_date
            .as_deref()
            .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .is_some_and(|d| d < today)
        {
            self.warn("overdue");
        }
    }
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            Kind::Nfe => "nfe",
            Kind::Nfce => "nfce",
            Kind::Cte => "cte",
            Kind::Event => "event",
            Kind::Boleto => "boleto",
            Kind::Pix => "pix",
            Kind::Receipt => "receipt",
        }
    }
}
pub(super) fn error(s: &str) -> CoreError {
    CoreError::Other(s.into())
}
pub(super) fn clean(s: &str) -> String {
    s.chars().filter(|c|!c.is_control()&&!matches!(*c,'\u{200b}'..='\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')).take(256).collect::<String>().trim().into()
}
pub(super) fn money(s: &str, brazilian: bool) -> Option<i64> {
    let s = s.trim().replace("R$", "").replace([' ', '\u{a0}'], "");
    let s = if brazilian {
        s.replace('.', "").replace(',', ".")
    } else {
        s
    };
    let (a, b) = s.split_once('.').unwrap_or((&s, ""));
    if a.is_empty()
        || !a.bytes().all(|c| c.is_ascii_digit())
        || b.len() > 2
        || !b.bytes().all(|c| c.is_ascii_digit())
        || a.len() > 12
    {
        return None;
    }
    a.parse::<i64>()
        .ok()?
        .checked_mul(100)?
        .checked_add(match b.len() {
            0 => 0,
            1 => b.parse::<i64>().ok()? * 10,
            _ => b.parse().ok()?,
        })
}
/// No URL resolution, QR image OCR, JavaScript execution, or external validation.
pub fn parse_bytes(
    name: &str,
    bytes: &[u8],
    reference: chrono::NaiveDate,
    body: &str,
) -> Result<Vec<FinancialDocument>> {
    if bytes.len() > MAX_INPUT {
        return Err(error("Document exceeds 16 MiB"));
    }
    let lower = name.to_ascii_lowercase();
    let mut docs = if bytes.starts_with(b"%PDF-") {
        let text = pdf::extract(bytes)?;
        text::parse(&text, reference)?
    } else if lower.ends_with(".xml")
        || bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) == Some(b'<')
    {
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(error("XML exceeds 4 MiB"));
        }
        vec![xml::parse(bytes)?]
    } else if lower.ends_with(".txt") || lower.ends_with(".csv") || lower.ends_with(".emv") {
        let text = std::str::from_utf8(bytes).map_err(|_| error("Document text must be UTF-8"))?;
        text::parse(text, reference)?
    } else {
        return Err(error("Supported documents: XML, text and text-bearing PDF"));
    };
    let values = text::body_amounts(body);
    for d in &mut docs {
        if body.len() > 64 * 1024 {
            d.warn("body_comparison_skipped_limit");
        }
        if values.len() == 1 && d.amount_cents.is_some_and(|a| !values.contains(&a)) {
            d.warn("body_amount_mismatch");
        } else if values.len() > 1 {
            d.warn("body_amount_ambiguous");
        }
        d.refresh_warnings(chrono::Local::now().date_naive());
    }
    docs.truncate(MAX_DOCUMENTS);
    Ok(docs)
}

#[cfg(test)]
mod tests;
