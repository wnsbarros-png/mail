use super::*;
use chrono::{Duration, NaiveDate};
/// Returns 255 for invalid/non-digit or overlong input.
pub fn mod10(s: &str) -> u8 {
    if s.is_empty() || s.len() > 1024 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return 255;
    }
    let sum: u32 = s
        .bytes()
        .rev()
        .zip([2, 1].into_iter().cycle())
        .map(|(c, w)| {
            let n = u32::from(c - b'0') * w;
            n / 10 + n % 10
        })
        .sum();
    ((10 - sum % 10) % 10) as u8
}
/// Returns 255 for invalid/non-digit or overlong input.
pub fn mod11(s: &str) -> u8 {
    if s.is_empty() || s.len() > 1024 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return 255;
    }
    let r: u32 = s
        .bytes()
        .rev()
        .zip((2..=9).cycle())
        .map(|(c, w)| u32::from(c - b'0') * w)
        .sum::<u32>()
        % 11;
    if r < 2 { 0 } else { (11 - r) as u8 }
}
/// Returns 255 for invalid/non-digit or overlong input.
pub fn bank_dv(s: &str) -> u8 {
    if s.is_empty() || s.len() > 1024 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return 255;
    }
    let r: u32 = s
        .bytes()
        .rev()
        .zip((2..=9).cycle())
        .map(|(c, w)| u32::from(c - b'0') * w)
        .sum::<u32>()
        % 11;
    match 11 - r {
        0 | 10 | 11 => 1,
        n => n as u8,
    }
}
pub fn validate_access_key(s: &str) -> bool {
    s.len() == 44
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.bytes().any(|b| b != b'0')
        && mod11(&s[..43]) == s.as_bytes()[43] - b'0'
}
pub fn crc16(s: &[u8]) -> u16 {
    let mut crc = 0xffff;
    for &byte in s {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}
/// Keep BOTH cycles: factor 1000 is 2000-07-03 OR 2025-02-22.
/// Printed date is retained literally; conflicting factor/date values are explicitly warned.
pub fn decode_boleto(
    input: &str,
    reference: NaiveDate,
    printed_due: Option<NaiveDate>,
) -> Result<FinancialDocument> {
    if input.len() > 256
        || input
            .chars()
            .any(|c| !c.is_ascii_digit() && !matches!(c, ' ' | '.' | '-' | '\n' | '\r' | '\t'))
    {
        return Err(error("Invalid boleto representation"));
    }
    let n: String = input
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(char::from)
        .collect();
    if !matches!(n.len(), 44 | 47 | 48) {
        return Err(error("Boleto needs a 44-digit barcode or 47/48-digit line"));
    }
    let mut doc = FinancialDocument::new(Kind::Boleto);
    doc.code = Some(n.clone());
    let mut blocks = true;
    let barcode = if n.len() == 47 {
        blocks = mod10(&n[..9]) == n.as_bytes()[9] - b'0'
            && mod10(&n[10..20]) == n.as_bytes()[20] - b'0'
            && mod10(&n[21..31]) == n.as_bytes()[31] - b'0';
        format!(
            "{}{}{}{}{}{}",
            &n[..4],
            &n[32..33],
            &n[33..],
            &n[4..9],
            &n[10..20],
            &n[21..31]
        )
    } else if n.len() == 48 {
        let mut b = String::new();
        for chunk in n.as_bytes().as_chunks::<12>().0 {
            let part = std::str::from_utf8(&chunk[..11]).unwrap();
            let dv = match n.as_bytes()[2] {
                b'6' | b'7' => mod10(part),
                b'8' | b'9' => mod11(part),
                _ => return Err(error("Invalid utility reference digit")),
            };
            blocks &= dv == chunk[11] - b'0';
            b.push_str(part);
        }
        b
    } else {
        n.clone()
    };
    if n.len() == 48 && !barcode.starts_with('8') {
        return Err(error("Boleto length/type mismatch"));
    }
    let general = if n.len() == 48 || (n.len() == 44 && barcode.starts_with('8')) {
        let raw = format!("{}{}", &barcode[..3], &barcode[4..]);
        let dv = match barcode.as_bytes()[2] {
            b'6' | b'7' => mod10(&raw),
            b'8' | b'9' => mod11(&raw),
            _ => return Err(error("Invalid utility reference digit")),
        };
        if matches!(barcode.as_bytes()[2], b'6' | b'8') {
            doc.amount_cents = barcode[4..15].parse().ok();
        } else {
            doc.warn("reference_value_not_currency");
        }
        doc.fields
            .insert("boleto_type".into(), "arrecadacao".into());
        doc.fields.insert("segment".into(), barcode[1..2].into());
        if let Some(date) = printed_due {
            doc.due_date = Some(date.to_string());
            doc.warn("printed_due_unverified");
        }
        dv == barcode.as_bytes()[3] - b'0'
    } else {
        if barcode.as_bytes()[3] != b'9' {
            doc.warn("unsupported_currency");
        } else {
            doc.amount_cents = barcode[9..19].parse().ok();
        }
        doc.fields.insert("boleto_type".into(), "bancario".into());
        doc.fields.insert("bank_code".into(), barcode[..3].into());
        let factor: u16 = barcode[5..9].parse().unwrap();
        doc.fields.insert("due_factor".into(), factor.to_string());
        if factor != 0 {
            let legacy =
                NaiveDate::from_ymd_opt(1997, 10, 7).unwrap() + Duration::days(i64::from(factor));
            let reset = NaiveDate::from_ymd_opt(2025, 2, 22).unwrap();
            let modern = (factor >= 1000).then(|| reset + Duration::days(i64::from(factor) - 1000));
            doc.due_candidates.push(legacy.to_string());
            if let Some(modern) = modern {
                doc.due_candidates.push(modern.to_string());
            }
            let due = if let Some(date) = printed_due {
                if date == legacy || modern == Some(date) {
                    date
                } else {
                    doc.warn("printed_due_factor_mismatch");
                    date
                }
            } else {
                doc.warn("due_date_inferred");
                if let Some(modern) = modern
                    && reference >= reset
                {
                    modern
                } else {
                    legacy
                }
            };
            if modern.is_some() && printed_due.is_none() {
                doc.warn("due_factor_cycle_ambiguous");
            }
            doc.due_date = Some(due.to_string());
        } else {
            doc.warn("no_due_factor");
            doc.due_date = printed_due.map(|d| d.to_string());
        }
        bank_dv(&format!("{}{}", &barcode[..4], &barcode[5..])) == barcode.as_bytes()[4] - b'0'
    };
    doc.fields.insert("barcode".into(), barcode);
    doc.checks.insert("block_dvs".into(), blocks);
    doc.checks.insert("general_dv".into(), general);
    if !blocks || !general {
        doc.warn("invalid_dv");
    }
    Ok(doc)
}
fn tlv(s: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let mut i = 0;
    while i < s.len() {
        if i + 4 > s.len()
            || !s.is_char_boundary(i + 4)
            || !s.as_bytes()[i..i + 4].iter().all(u8::is_ascii_digit)
        {
            return Err(error("Invalid EMV TLV header"));
        }
        let len = s[i + 2..i + 4]
            .parse::<usize>()
            .map_err(|_| error("Invalid EMV length"))?;
        let end = i + 4 + len;
        if end > s.len() || !s.is_char_boundary(end) {
            return Err(error("Invalid EMV byte length"));
        }
        if out
            .insert(s[i..i + 2].into(), s[i + 4..end].into())
            .is_some()
        {
            return Err(error("Duplicate EMV field"));
        }
        i = end;
        if out.len() > 64 {
            return Err(error("Too many EMV fields"));
        }
    }
    Ok(out)
}
pub fn decode_pix(s: &str) -> Result<FinancialDocument> {
    if s.len() > 8192 || !s.starts_with("000201") || s.len() < 12 {
        return Err(error("Invalid PIX payload"));
    }
    let fields = tlv(s)?;
    let suffix = s
        .get(s.len() - 8..)
        .ok_or_else(|| error("Invalid PIX suffix"))?;
    if !suffix.starts_with("6304") || !suffix[4..].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(error("PIX CRC must be the final field"));
    }
    let mut merchant = None;
    for (id, value) in &fields {
        if let Ok(id) = id.parse::<u8>()
            && (26..=51).contains(&id)
        {
            let nested = tlv(value)?;
            if nested
                .get("00")
                .is_some_and(|g| g.eq_ignore_ascii_case("br.gov.bcb.pix"))
            {
                merchant = Some(nested);
                break;
            }
        }
    }
    let merchant = merchant.ok_or_else(|| error("EMV payload is not PIX"))?;
    if fields.get("53").map(String::as_str) != Some("986")
        || fields.get("58").map(String::as_str) != Some("BR")
    {
        return Err(error("Unsupported PIX country/currency"));
    }
    let mut doc = FinancialDocument::new(Kind::Pix);
    let crc = crc16(&s.as_bytes()[..s.len() - 4]);
    let valid = u16::from_str_radix(&suffix[4..], 16).ok() == Some(crc);
    doc.checks.insert("crc16".into(), valid);
    if !valid {
        doc.warn("invalid_crc16");
    }
    doc.code = Some(s.into());
    doc.amount_cents = fields.get("54").and_then(|v| money(v, false));
    if fields.contains_key("54") && doc.amount_cents.is_none() {
        doc.warn("invalid_amount");
    }
    doc.recipient = Some(Party {
        cnpj: None,
        name: fields.get("59").map(|s| clean(s)),
    });
    for (key, label) in [("01", "pix_key"), ("25", "dynamic_url_not_fetched")] {
        if let Some(v) = merchant.get(key) {
            doc.fields.insert(label.into(), clean(v));
        }
    }
    if let Some(v) = fields.get("62") {
        let extra = tlv(v)?;
        if let Some(txid) = extra.get("05") {
            doc.fields.insert("txid".into(), clean(txid));
        }
    }
    doc.warn("pix_recipient_not_verified");
    Ok(doc)
}
