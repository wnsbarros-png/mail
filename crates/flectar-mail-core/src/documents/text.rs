use super::*;
use chrono::NaiveDate;
use once_cell::sync::Lazy;
use regex::Regex;
static AMOUNT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"R\$\s*([0-9]{1,3}(?:\.[0-9]{3})*,[0-9]{2}|[0-9]{1,12},[0-9]{2})\b").unwrap()
});
static DATE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b([0-9]{2})/([0-9]{2})/([0-9]{4})\b").unwrap());
static E2E: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[ED][0-9]{16}[a-zA-Z0-9]{15}\b").unwrap());
static BANK_LINE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"\b[0-9]{5}\.[0-9]{5}\s+[0-9]{5}\.[0-9]{6}\s+[0-9]{5}\.[0-9]{6}\s+[0-9]\s+[0-9]{14}\b",
    )
    .unwrap()
});
static UTILITY_LINE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b[0-9]{11}[- ]?[0-9]\s+[0-9]{11}[- ]?[0-9]\s+[0-9]{11}[- ]?[0-9]\s+[0-9]{11}[- ]?[0-9]\b").unwrap()
});
static RAW_CODE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b(?:[0-9]{48}|[0-9]{47}|[0-9]{44})\b").unwrap());
static PIX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)000201.{0,4096}?6304[0-9A-Fa-f]{4}").unwrap());
pub(super) fn body_amounts(s: &str) -> std::collections::BTreeSet<i64> {
    if s.len() > 64 * 1024 {
        return Default::default();
    }
    AMOUNT
        .captures_iter(s)
        .take(32)
        .filter_map(|c| money(&c[1], true))
        .collect()
}
fn date(s: &str) -> Option<NaiveDate> {
    let c = DATE.captures(s)?;
    NaiveDate::from_ymd_opt(c[3].parse().ok()?, c[2].parse().ok()?, c[1].parse().ok()?)
}
fn normalized(s: &str) -> String {
    deunicode::deunicode(s)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn labels(s: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let lines = s.lines().take(5000).collect::<Vec<_>>();
    for (i, line) in lines.iter().enumerate() {
        let (k, v) = line.split_once(':').unwrap_or((line, ""));
        let key = normalized(k.trim());
        let category = if matches!(
            key.as_str(),
            "valor"
                | "valor pago"
                | "valor total"
                | "valor do documento"
                | "valor da transferencia"
                | "valor da transacao"
        ) {
            Some("amount")
        } else if key.contains("vencimento") {
            Some("due")
        } else if matches!(
            key.as_str(),
            "data"
                | "data do pagamento"
                | "data da transferencia"
                | "data da transacao"
                | "data e hora"
        ) {
            Some("date")
        } else if key.contains("beneficiario")
            || key.contains("favorecido")
            || key.contains("recebedor")
        {
            Some("recipient")
        } else if key.contains("pagador") || key.contains("remetente") {
            Some("payer")
        } else if key.contains("identificador")
            || key.contains("id da transacao")
            || key == "autenticacao"
        {
            Some("transaction_id")
        } else {
            None
        };
        if let Some(category) = category {
            let value = if v.trim().is_empty() {
                lines.get(i + 1).copied().unwrap_or_default()
            } else {
                v
            };
            if !value.trim().is_empty() {
                map.entry(category.into()).or_insert_with(|| clean(value));
            }
        }
    }
    map
}
fn labeled_amount(s: &str) -> Option<i64> {
    AMOUNT
        .captures(s)
        .and_then(|c| money(&c[1], true))
        .or_else(|| money(s, true))
}
pub(super) fn parse(s: &str, reference: NaiveDate) -> Result<Vec<FinancialDocument>> {
    if s.len() > MAX_TEXT {
        return Err(error("Extracted text exceeds 1 MiB"));
    }
    let labels = labels(s);
    let due = labels.get("due").and_then(|s| date(s));
    let amount = labels.get("amount").and_then(|s| labeled_amount(s));
    let mut docs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for regex in [&*BANK_LINE, &*UTILITY_LINE, &*RAW_CODE] {
        for candidate in regex.find_iter(s).take(32) {
            if let Ok(mut d) = decode_boleto(candidate.as_str(), reference, due) {
                let code = d.fields.get("barcode").cloned().unwrap_or_default();
                if seen.insert(code) {
                    if let Some(name) = labels.get("recipient") {
                        d.recipient = Some(Party {
                            cnpj: None,
                            name: Some(name.clone()),
                        });
                    }
                    if amount.zip(d.amount_cents).is_some_and(|(a, b)| a != b) {
                        d.warn("printed_amount_mismatch");
                    }
                    docs.push(d);
                    if docs.len() >= MAX_DOCUMENTS {
                        return Ok(docs);
                    }
                }
            }
        }
    }
    let joined = s.replace(['\r', '\n'], "");
    for candidate in PIX.find_iter(&joined).take(16) {
        if let Ok(d) = decode_pix(candidate.as_str())
            && seen.insert(candidate.as_str().into())
        {
            docs.push(d);
            if docs.len() >= MAX_DOCUMENTS {
                return Ok(docs);
            }
        }
    }
    let norm = normalized(s);
    if [
        "comprovante",
        "pagamento realizado",
        "transferencia realizada",
        "pix enviado",
        "ted efetuada",
    ]
    .iter()
    .any(|k| norm.contains(k))
    {
        let mut d = FinancialDocument::new(Kind::Receipt);
        d.confidence = if amount.is_some() && labels.contains_key("date") {
            "heuristic_medium"
        } else {
            "heuristic_low"
        }
        .into();
        d.amount_cents = amount;
        d.issued_at = labels
            .get("date")
            .and_then(|s| date(s))
            .map(|d| d.to_string());
        d.recipient = labels.get("recipient").map(|s| Party {
            cnpj: None,
            name: Some(s.clone()),
        });
        d.issuer = labels.get("payer").map(|s| Party {
            cnpj: None,
            name: Some(s.clone()),
        });
        if let Some(e) = E2E.find(s) {
            d.fields.insert("e2e".into(), e.as_str().into());
        }
        if let Some(id) = labels.get("transaction_id") {
            d.fields.insert("transaction_id".into(), id.clone());
        }
        if let Some(date) = labels.get("date") {
            d.fields.insert("date_literal".into(), date.clone());
        }
        d.fields.insert(
            "receipt_type".into(),
            if norm.contains("pix") {
                "pix"
            } else if norm.contains("ted") {
                "ted"
            } else if norm.contains("boleto") {
                "boleto"
            } else {
                "payment"
            }
            .into(),
        );
        d.warn("receipt_is_heuristic_not_payment_confirmation");
        docs.push(d);
    }
    docs.truncate(MAX_DOCUMENTS);
    Ok(docs)
}
