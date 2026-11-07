use super::*;
use roxmltree::Node;
fn child<'a, 'b>(node: Node<'a, 'b>, name: &str) -> Option<Node<'a, 'b>> {
    node.children().find(|n| {
        n.is_element()
            && n.tag_name().name() == name
            && n.tag_name().namespace() == node.tag_name().namespace()
    })
}
fn path<'a, 'b>(node: Node<'a, 'b>, names: &[&str]) -> Option<Node<'a, 'b>> {
    names.iter().try_fold(node, |n, name| child(n, name))
}
fn value(node: Node<'_, '_>, names: &[&str]) -> Option<String> {
    path(node, names)?
        .text()
        .map(clean)
        .filter(|s| !s.is_empty())
}
fn party(node: Node<'_, '_>, name: &str) -> Option<Party> {
    let n = child(node, name)?;
    Some(Party {
        cnpj: value(n, &["CNPJ"]),
        name: value(n, &["xNome"]),
    })
}
pub(super) fn parse(bytes: &[u8]) -> Result<FinancialDocument> {
    let input = std::str::from_utf8(bytes).map_err(|_| error("Fiscal XML must be UTF-8"))?;
    // Streaming preflight bounds depth before building a tree; DOCTYPE/entities are refused.
    let mut reader = quick_xml::Reader::from_str(input);
    let mut depth = 0u32;
    let mut nodes = 0u32;
    loop {
        use quick_xml::events::Event;
        match reader.read_event().map_err(|_| error("Malformed XML"))? {
            Event::DocType(_) => {
                return Err(error("XML DOCTYPE and external entities are forbidden"));
            }
            Event::Start(_) => {
                depth += 1;
                nodes += 1;
            }
            Event::Empty(_) => nodes += 1,
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| error("Invalid XML nesting"))?;
            }
            Event::Eof => break,
            _ => {}
        }
        if depth > 64 || nodes > 50_000 {
            return Err(error("XML depth/node limit exceeded"));
        }
    }
    let tree = roxmltree::Document::parse_with_options(
        input,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 100_000,
            ..Default::default()
        },
    )
    .map_err(|_| error("Invalid fiscal XML (DTD disabled)"))?;
    let root = tree.root_element();
    let ns = root.tag_name().namespace().unwrap_or_default();
    let cte = ns == "http://www.portalfiscal.inf.br/cte";
    if !cte && ns != "http://www.portalfiscal.inf.br/nfe" {
        return Err(error("Unrecognized fiscal XML namespace"));
    }
    if matches!(
        root.tag_name().name(),
        "procEventoNFe" | "evento" | "procEventoCTe" | "eventoCTe"
    ) {
        let event = if root.tag_name().name().starts_with("proc") {
            child(root, if cte { "eventoCTe" } else { "evento" })
        } else {
            Some(root)
        }
        .and_then(|n| child(n, "infEvento"))
        .ok_or_else(|| error("Missing fiscal event"))?;
        let mut d = FinancialDocument::new(Kind::Event);
        d.access_key = value(event, &[if cte { "chCTe" } else { "chNFe" }]);
        d.issued_at = value(event, &["dhEvento"]);
        d.issuer = Some(Party {
            cnpj: value(event, &["CNPJ"]),
            name: None,
        });
        if let Some(code) = value(event, &["tpEvento"]) {
            d.fields.insert(
                "event_type".into(),
                match code.as_str() {
                    "110111" => "cancelamento",
                    "110110" => "carta_correcao",
                    _ => "outro_evento",
                }
                .into(),
            );
            d.fields.insert("event_code".into(), code);
        }
        if let Some(description) = value(event, &["detEvento", "descEvento"]) {
            d.fields.insert("description".into(), description);
        }
        let ret = path(
            root,
            &[if cte { "retEventoCTe" } else { "retEvento" }, "infEvento"],
        );
        if let Some(ret) = ret {
            d.protocol = value(ret, &["nProt"]);
            d.status = value(ret, &["cStat"]);
            if value(ret, &[if cte { "chCTe" } else { "chNFe" }])
                .is_some_and(|k| Some(k) != d.access_key)
            {
                d.warn("protocol_key_mismatch");
            }
        }
        let valid = d.access_key.as_deref().is_some_and(validate_access_key);
        d.checks.insert("access_key_dv".into(), valid);
        if !valid {
            d.warn("invalid_access_key");
        }
        d.warn("event_signature_not_verified");
        return Ok(d);
    }
    let base = if root.tag_name().name() == if cte { "CTe" } else { "NFe" } {
        root
    } else {
        child(root, if cte { "CTe" } else { "NFe" })
            .ok_or_else(|| error("XML is not NFe/NFCe/CTe"))?
    };
    let info = child(base, if cte { "infCte" } else { "infNFe" })
        .ok_or_else(|| error("Missing fiscal document details"))?;
    if info.attribute("versao") != Some("4.00") {
        return Err(error("Only fiscal XML layout 4.00 is supported"));
    }
    let model = value(info, &["ide", "mod"]);
    let mut d = FinancialDocument::new(if cte {
        Kind::Cte
    } else if model.as_deref() == Some("65") {
        Kind::Nfce
    } else {
        Kind::Nfe
    });
    d.issuer = party(info, "emit");
    d.recipient = party(info, "dest");
    d.number = value(info, &["ide", if cte { "nCT" } else { "nNF" }]);
    d.series = value(info, &["ide", "serie"]);
    d.issued_at = value(info, &["ide", "dhEmi"]);
    d.access_key = info
        .attribute("Id")
        .and_then(|s| s.strip_prefix(if cte { "CTe" } else { "NFe" }))
        .map(clean);
    let valid = d.access_key.as_deref().is_some_and(validate_access_key);
    d.checks.insert("access_key_dv".into(), valid);
    if !valid {
        d.warn("invalid_access_key");
    }
    d.amount_cents = value(
        info,
        if cte {
            &["vPrest", "vTPrest"]
        } else {
            &["total", "ICMSTot", "vNF"]
        },
    )
    .and_then(|s| money(&s, false));
    if d.issuer.is_none() || d.number.is_none() || d.amount_cents.is_none() || d.issued_at.is_none()
    {
        d.warn("missing_fields");
    }
    if d.issued_at
        .as_deref()
        .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_err())
    {
        d.warn("invalid_issue_date");
    }
    if let Some(key) = d
        .access_key
        .as_deref()
        .filter(|k| k.len() == 44 && k.is_ascii())
    {
        let series = d
            .series
            .as_deref()
            .and_then(|s| s.parse::<u32>().ok())
            .map(|n| format!("{n:03}"));
        let number = d
            .number
            .as_deref()
            .and_then(|s| s.parse::<u64>().ok())
            .map(|n| format!("{n:09}"));
        let mismatches = d
            .issuer
            .as_ref()
            .and_then(|p| p.cnpj.as_deref())
            .is_some_and(|c| c != &key[6..20])
            || series.is_some_and(|s| s != key[22..25])
            || number.is_some_and(|s| s != key[25..34])
            || model.is_some_and(|s| s != key[20..22]);
        if mismatches {
            d.warn("access_key_fields_mismatch");
        }
    }
    let protocol = path(root, &[if cte { "protCTe" } else { "protNFe" }, "infProt"]);
    if let Some(p) = protocol {
        d.protocol = value(p, &["nProt"]);
        d.status = value(p, &["cStat"]);
        if value(p, &[if cte { "chCTe" } else { "chNFe" }]).is_some_and(|k| Some(k) != d.access_key)
        {
            d.warn("protocol_key_mismatch");
        }
    } else {
        d.warn("no_authorization_protocol");
    }
    for item in info.children().filter(|n| {
        n.is_element() && n.tag_name().name() == "det" && n.tag_name().namespace() == Some(ns)
    }) {
        d.item_count += 1;
        if d.items.len() < 20
            && let Some(p) = child(item, "prod")
        {
            d.items.push(Item {
                code: value(p, &["cProd"]),
                description: value(p, &["xProd"]).unwrap_or_default(),
                quantity: value(p, &["qCom"]),
                unit: value(p, &["uCom"]),
                amount_cents: value(p, &["vProd"]).and_then(|s| money(&s, false)),
            });
        }
    }
    if d.item_count > d.items.len() {
        d.warn("items_truncated");
    }
    d.warn("xml_signature_not_verified");
    Ok(d)
}
