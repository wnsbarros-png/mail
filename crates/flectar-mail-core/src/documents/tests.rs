use super::*;
use chrono::NaiveDate;
fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}
fn key() -> String {
    let raw = "3526110000000000000055001000000001100000001";
    format!("{}{}", raw, mod11(raw))
}
fn xml(model: &str) -> String {
    format!(
        r#"<nfeProc xmlns="http://www.portalfiscal.inf.br/nfe" versao="4.00"><NFe><infNFe Id="NFe{}" versao="4.00"><ide><mod>{model}</mod><nNF>1</nNF><serie>1</serie><dhEmi>2026-11-07T08:00:00-03:00</dhEmi></ide><emit><CNPJ>00000000000000</CNPJ><xNome>EMITENTE SINTÉTICO</xNome></emit><dest><CNPJ>00000000000000</CNPJ><xNome>DESTINATÁRIO SINTÉTICO</xNome></dest><det nItem="1"><prod><cProd>TESTE</cProd><xProd>Item sintético</xProd><qCom>2.0000</qCom><uCom>UN</uCom><vProd>123.45</vProd></prod></det><total><ICMSTot><vNF>123.45</vNF></ICMSTot></total></infNFe></NFe><protNFe><infProt><chNFe>{}</chNFe><nProt>000000000000000</nProt><cStat>100</cStat></infProt></protNFe></nfeProc>"#,
        key(),
        key()
    )
}
fn bank(factor: u16) -> String {
    let mut raw = format!("00190{factor:04}0000012345{}", "0".repeat(25));
    let dv = bank_dv(&format!("{}{}", &raw[..4], &raw[5..]));
    raw.replace_range(4..5, &dv.to_string());
    raw
}
fn line47(bar: &str) -> String {
    let a = format!("{}{}", &bar[..4], &bar[19..24]);
    let b = &bar[24..34];
    let c = &bar[34..];
    format!(
        "{}{}{}{}{}{}{}{}",
        a,
        mod10(&a),
        b,
        mod10(b),
        c,
        mod10(c),
        &bar[4..5],
        &bar[5..19]
    )
}
fn utility(reference: u8) -> String {
    let mut bar = format!("81{reference}0{}{}", "00000012345", "0".repeat(29));
    let raw = format!("{}{}", &bar[..3], &bar[4..]);
    let dv = if reference < 8 {
        mod10(&raw)
    } else {
        mod11(&raw)
    };
    bar.replace_range(3..4, &dv.to_string());
    bar
}
fn line48(bar: &str) -> String {
    bar.as_bytes()
        .as_chunks::<11>()
        .0
        .iter()
        .map(|c| {
            let s = std::str::from_utf8(c).unwrap();
            format!(
                "{}{}",
                s,
                if bar.as_bytes()[2] < b'8' {
                    mod10(s)
                } else {
                    mod11(s)
                }
            )
        })
        .collect()
}
#[test]
fn fiscal_fields_items_protocol_and_dv() {
    let docs = parse_bytes(
        "synthetic.xml",
        xml("55").as_bytes(),
        day("2026-11-07"),
        "Total R$ 123,45",
    )
    .unwrap();
    let d = &docs[0];
    assert_eq!(d.kind, Kind::Nfe);
    assert_eq!(d.amount_cents, Some(12345));
    assert!(d.checks["access_key_dv"]);
    assert_eq!(d.number.as_deref(), Some("1"));
    assert_eq!(d.series.as_deref(), Some("1"));
    assert_eq!(d.item_count, 1);
    assert_eq!(d.items[0].quantity.as_deref(), Some("2.0000"));
    assert_eq!(d.status.as_deref(), Some("100"));
    assert!(d.warnings.contains(&"xml_signature_not_verified".into()));
    assert!(!d.warnings.contains(&"body_amount_mismatch".into()));
}
#[test]
fn nfce_and_invalid_key_are_not_authenticated() {
    let mut s = xml("65");
    s = s.replace(&key(), &"1".repeat(44));
    let d = xml::parse(s.as_bytes()).unwrap();
    assert_eq!(d.kind, Kind::Nfce);
    assert!(!d.checks["access_key_dv"]);
    assert!(d.warnings.contains(&"invalid_access_key".into()));
    assert_eq!(d.confidence, "structured_unverified");
}
#[test]
fn xml_rejects_entities_namespaces_versions_depth_and_size() {
    for s in [
        format!(
            "<!DOCTYPE x [<!ENTITY secret SYSTEM 'file:///etc/passwd'>]>{}",
            xml("55")
        ),
        xml("55").replace("http://www.portalfiscal.inf.br/nfe", "https://evil.invalid"),
        xml("55").replace("4.00", "3.10"),
        "<x>".repeat(65) + &"</x>".repeat(65),
        "<!DOCTYPE x [<!ENTITY x 'a'>]><x>&x;</x>".into(),
    ] {
        assert!(parse_bytes("a.xml", s.as_bytes(), day("2026-11-07"), "").is_err());
    }
    assert!(
        parse_bytes(
            "a.xml",
            &vec![b' '; 4 * 1024 * 1024 + 1],
            day("2026-11-07"),
            ""
        )
        .is_err()
    );
}
#[test]
fn event_and_cte_detection() {
    let s = format!(
        r#"<procEventoNFe xmlns="http://www.portalfiscal.inf.br/nfe"><evento><infEvento><chNFe>{}</chNFe><tpEvento>110111</tpEvento><dhEvento>2026-11-07T09:00:00-03:00</dhEvento></infEvento></evento></procEventoNFe>"#,
        key()
    );
    let d = xml::parse(s.as_bytes()).unwrap();
    assert_eq!(d.kind, Kind::Event);
    assert_eq!(d.fields["event_type"], "cancelamento");
    let s = format!(
        r#"<CTe xmlns="http://www.portalfiscal.inf.br/cte"><infCte versao="4.00" Id="CTe{}"><ide><mod>57</mod><nCT>1</nCT><serie>1</serie></ide><vPrest><vTPrest>10.00</vTPrest></vPrest></infCte></CTe>"#,
        key()
    );
    let d = xml::parse(s.as_bytes()).unwrap();
    assert_eq!(d.kind, Kind::Cte);
    assert_eq!(d.amount_cents, Some(1000));
}
#[test]
fn factor_reset_and_legacy_boundaries() {
    for (factor, reference, expected) in [
        (9999, "2025-02-21", "2025-02-21"),
        (1000, "2025-02-22", "2025-02-22"),
        (1001, "2025-02-23", "2025-02-23"),
        (1000, "2000-07-03", "2000-07-03"),
    ] {
        let d = decode_boleto(&bank(factor), day(reference), None).unwrap();
        assert_eq!(d.due_date.as_deref(), Some(expected));
        assert!(d.checks["general_dv"]);
    }
    let d = decode_boleto(&bank(1000), day("2026-11-07"), Some(day("2000-07-03"))).unwrap();
    assert_eq!(d.due_date.as_deref(), Some("2000-07-03"));
    assert_eq!(d.due_candidates, vec!["2000-07-03", "2025-02-22"]);
    assert_eq!(
        decode_boleto(&bank(0), day("2026-11-07"), None)
            .unwrap()
            .due_date,
        None
    );
}
#[test]
fn bank_47_dvs_and_printed_conflict() {
    let bar = bank(1000);
    let line = line47(&bar);
    assert_eq!(line.len(), 47);
    let mut d = decode_boleto(&line, day("2025-02-22"), Some(day("2026-11-07"))).unwrap();
    assert!(d.checks["general_dv"] && d.checks["block_dvs"]);
    assert_eq!(d.amount_cents, Some(12345));
    assert_eq!(d.fields["bank_code"], "001");
    assert!(d.warnings.contains(&"printed_due_factor_mismatch".into()));
    let mut bad = line.clone();
    bad.replace_range(9..10, &((line.as_bytes()[9] - b'0' + 1) % 10).to_string());
    assert!(!decode_boleto(&bad, day("2026-11-07"), None).unwrap().checks["block_dvs"]);
    d.refresh_warnings(day("2026-11-08"));
    assert!(d.warnings.contains(&"overdue".into()));
}
#[test]
fn utility_mod10_and_mod11_reference_and_actual_value() {
    for reference in [6, 7, 8, 9] {
        let bar = utility(reference);
        let line = line48(&bar);
        assert_eq!(line.len(), 48);
        let d = decode_boleto(&line, day("2026-11-07"), None).unwrap();
        assert!(
            d.checks["general_dv"] && d.checks["block_dvs"],
            "{reference}"
        );
        assert_eq!(
            d.amount_cents,
            if reference == 6 || reference == 8 {
                Some(12345)
            } else {
                None
            }
        );
        let mut bad = line;
        bad.replace_range(11..12, "9");
        if bad != line48(&bar) {
            assert!(!decode_boleto(&bad, day("2026-11-07"), None).unwrap().checks["block_dvs"]);
        }
    }
}
const PIX: &str = "00020126580014br.gov.bcb.pix0136123e4567-e12b-12d1-a456-4266554400005204000053039865802BR5913Fulano de Tal6008BRASILIA62070503***63041D3D";
#[test]
fn pix_crc_bcb_vector_and_mutations() {
    assert_eq!(crc16(b"123456789"), 0x29B1);
    let d = decode_pix(PIX).unwrap();
    assert!(d.checks["crc16"]);
    assert_eq!(d.fields["txid"], "***");
    assert_eq!(d.amount_cents, None);
    let bad = PIX.replace("1D3D", "0000");
    assert!(!decode_pix(&bad).unwrap().checks["crc16"]);
    assert!(decode_pix(&PIX.replace("2658", "2699")).is_err());
    for n in 0..PIX.len() {
        let prefix = &PIX[..n];
        let _ = decode_pix(prefix);
    }
    for s in ["000201éééé", "0002016304🦀", "0002016304FFFF000201"] {
        assert!(decode_pix(s).is_err());
    }
}
#[test]
fn code_hardening_no_panics_and_no_non_digits() {
    for s in ["", "é", "🦀", "-1", "\0", &"9".repeat(1025)] {
        assert_eq!(mod10(s), 255);
        assert_eq!(mod11(s), 255);
        assert_eq!(bank_dv(s), 255);
        assert!(!validate_access_key(s));
        assert!(decode_boleto(s, day("2026-11-07"), None).is_err());
    }
    assert!(!validate_access_key(&"0".repeat(44)));
    assert!(validate_access_key(&key()));
}
#[test]
fn receipt_extraction_is_explicitly_heuristic() {
    let s = "Comprovante PIX\nValor pago: R$ 123,45\nData: 07/11/2026 08:00\nPagador: PESSOA SINTÉTICA\nRecebedor: EMPRESA SINTÉTICA\nE0000000020261107000000000000000";
    let d = text::parse(s, day("2026-11-07")).unwrap();
    assert_eq!(d[0].kind, Kind::Receipt);
    assert_eq!(d[0].amount_cents, Some(12345));
    assert_eq!(d[0].confidence, "heuristic_medium");
    assert_eq!(d[0].issued_at.as_deref(), Some("2026-11-07"));
    assert!(
        d[0].warnings
            .contains(&"receipt_is_heuristic_not_payment_confirmation".into())
    );
    assert_eq!(
        d[0].recipient.as_ref().unwrap().name.as_deref(),
        Some("EMPRESA SINTÉTICA")
    );
}
#[test]
fn body_mismatch_and_ambiguous_are_warnings() {
    let d = parse_bytes(
        "a.xml",
        xml("55").as_bytes(),
        day("2026-11-07"),
        "R$ 999,99",
    )
    .unwrap();
    assert!(d[0].warnings.contains(&"body_amount_mismatch".into()));
    let d = parse_bytes(
        "a.xml",
        xml("55").as_bytes(),
        day("2026-11-07"),
        "R$ 999,99 e R$ 123,45",
    )
    .unwrap();
    assert!(d[0].warnings.contains(&"body_amount_ambiguous".into()));
}
#[test]
fn text_finds_formatted_and_raw_codes_without_joining_unrelated_numbers() {
    let line = line47(&bank(1000));
    let s = format!("Boleto\n{line}\nVencimento: 22/02/2025\nBeneficiário: SINTÉTICO");
    let d = text::parse(&s, day("2025-02-22")).unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].code.as_ref(), Some(&line));
    assert!(
        text::parse("12345 67890 12345 67890", day("2026-11-07"))
            .unwrap()
            .is_empty()
    );
}
#[test]
fn money_is_integer_and_invalid_formats_refused() {
    assert_eq!(money("1.234,56", true), Some(123456));
    assert_eq!(money("1234.56", false), Some(123456));
    assert_eq!(money("1.2", false), Some(120));
    for s in ["-1", "NaN", "1.234", "1e9", "", "9999999999999999"] {
        assert_eq!(money(s, false), None);
    }
}
#[test]
fn pdf_text_fixture_and_no_text_or_malformed_refused() {
    use lopdf::{
        Document, Object, Stream,
        content::{Content, Operation},
        dictionary,
    };
    let mut pdf = Document::with_version("1.5");
    let pages = pdf.new_object_id();
    let font =
        pdf.add_object(dictionary! {"Type"=>"Font","Subtype"=>"Type1","BaseFont"=>"Helvetica"});
    let resources = pdf.add_object(dictionary! {"Font"=>dictionary!{"F1"=>font}});
    let content = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 12.into()]),
            Operation::new("Td", vec![50.into(), 750.into()]),
            Operation::new(
                "Tj",
                vec![Object::string_literal(format!(
                    "Comprovante PIX Valor: R$ 123,45 {}",
                    line47(&bank(1000))
                ))],
            ),
            Operation::new("ET", vec![]),
        ],
    };
    let content = pdf.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page=pdf.add_object(dictionary!{"Type"=>"Page","Parent"=>pages,"Contents"=>content,"Resources"=>resources,"MediaBox"=>vec![0.into(),0.into(),595.into(),842.into()]});
    pdf.objects.insert(
        pages,
        dictionary! {"Type"=>"Pages","Kids"=>vec![page.into()],"Count"=>1}.into(),
    );
    let catalog = pdf.add_object(dictionary! {"Type"=>"Catalog","Pages"=>pages});
    pdf.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    let d = parse_bytes("synthetic.pdf", &bytes, day("2026-11-07"), "").unwrap();
    assert!(d.iter().any(|d| d.kind == Kind::Boleto));
    assert!(parse_bytes("a.pdf", b"%PDF-1.7 corrupted", day("2026-11-07"), "").is_err());
}
