use super::*;
/// Rust-only parser. No external program, PDFium, scripts, URLs or OCR.
pub(super) fn extract(bytes: &[u8]) -> Result<String> {
    if bytes.windows(9).filter(|w| *w == b"endstream").count() > 64
        || bytes.windows(6).filter(|w| *w == b"endobj").count() > 5000
    {
        return Err(error("PDF exceeds stream/object budget"));
    }
    let mut doc = lopdf::Document::load_mem_with_options(
        bytes,
        lopdf::LoadOptions {
            max_decompressed_size: Some(2 * 1024 * 1024),
            ..Default::default()
        },
    )
    .map_err(|_| error("PDF malformed, encrypted or exceeds decompression limits"))?;
    if doc.was_encrypted() || doc.objects.len() > 30_000 {
        return Err(error("Encrypted or over-complex PDF refused"));
    }
    // Decode every stream with a limit before font/CMap extraction can use an unbounded API.
    let mut streams = 0;
    let mut decoded = 0usize;
    for object in doc.objects.values_mut() {
        if let lopdf::Object::Stream(stream) = object {
            streams += 1;
            if streams > 64 {
                return Err(error("PDF exceeds 64 streams"));
            }
            stream
                .decompress_with_limit(2 * 1024 * 1024)
                .map_err(|_| error("Unsupported or over-limit PDF stream"))?;
            decoded = decoded.saturating_add(stream.content.len());
            if decoded > 16 * 1024 * 1024 {
                return Err(error("PDF exceeds 16 MiB decoded stream budget"));
            }
        }
    }
    let pages = doc.get_pages();
    if pages.len() > 20 {
        return Err(error("PDF exceeds 20 pages"));
    }
    let mut out = String::new();
    for page in pages.keys() {
        let text = doc
            .extract_text_with_limit(&[*page], 2 * 1024 * 1024)
            .map_err(|_| error("PDF text extraction failed or decompression limit exceeded"))?;
        if out.len() + text.len() > MAX_TEXT {
            return Err(error("PDF extracted text exceeds 1 MiB"));
        }
        out.push_str(&text);
        out.push('\n');
    }
    if out.trim().is_empty() {
        return Err(error("PDF has no extractable text; OCR is not supported"));
    }
    Ok(out)
}
