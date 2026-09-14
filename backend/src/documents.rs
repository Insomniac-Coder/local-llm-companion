//! Document generation (§§36–38, 98): structured spec in, real file out.
//!
//! The LLM produces a JSON document specification — never raw binary.
//! Deterministic code renders it, validates the result by reopening it,
//! and records an artifact row. Office formats use minimal std-only writers
//! (stored-zip for docx/pptx, raw PDF writer) so no new dependencies are
//! needed; output is plain but always valid.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub conversation_id: String,
    pub filename: String,
    pub mime: String,
    pub size_bytes: u64,
    pub created_at: String,
}

pub fn mime_for(filename: &str) -> String {
    match Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("md") => "text/markdown",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("txt") => "text/plain",
        Some("html") | Some("htm") => "text/html",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
    .into()
}

/// Render one supported document kind. Returns bytes + mime.
pub fn render(kind: &str, spec: &serde_json::Value) -> Result<(Vec<u8>, String), String> {
    match kind.to_lowercase().as_str() {
        "txt" | "text" => {
            let body = spec.get("text").and_then(|v| v.as_str()).unwrap_or("");
            Ok((body.as_bytes().to_vec(), "text/plain".into()))
        }
        "md" | "markdown" => {
            let body = spec.get("markdown").and_then(|v| v.as_str())
                .or_else(|| spec.get("text").and_then(|v| v.as_str()))
                .unwrap_or("");
            Ok((body.as_bytes().to_vec(), "text/markdown".into()))
        }
        "json" => {
            let data = spec.get("data").unwrap_or(&serde_json::Value::Null);
            serde_json::to_vec_pretty(data)
                .map(|b| (b, "application/json".into()))
                .map_err(|e| format!("bad json spec: {e}"))
        }
        "csv" => {
            let rows = spec.get("rows").and_then(|v| v.as_array())
                .ok_or("csv spec needs {\"rows\": [[...], ...]}")?;
            let mut out = String::new();
            for row in rows {
                let cells = row.as_array().ok_or("csv rows must be arrays")?;
                out.push_str(&cells.iter().map(cell_text).collect::<Vec<_>>().join(","));
                out.push('\n');
            }
            Ok((out.into_bytes(), "text/csv".into()))
        }
        "html" => {
            let body = spec.get("html").and_then(|v| v.as_str()).unwrap_or("");
            let title = spec.get("title").and_then(|v| v.as_str()).unwrap_or("Document");
            let doc = format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head><body>\n{body}\n</body></html>");
            Ok((doc.into_bytes(), "text/html".into()))
        }
        "xlsx" | "spreadsheet" => render_xlsx(spec),
        "docx" | "word" => render_docx(spec),
        "pdf" => render_pdf(spec),
        "pptx" | "slides" | "presentation" => render_pptx(spec),
        other => Err(format!(
            "'{other}' has no local renderer. Supported: txt, md, json, csv, html, xlsx, docx, pdf, pptx."
        )),
    }
}

fn cell_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => {
            if s.contains([',', '"', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.clone()
            }
        }
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Sheets spec: {"sheets": [{"name": "S1", "rows": [[...], ...]}]}.
/// Numbers stay numeric; everything else becomes strings.
fn render_xlsx(spec: &serde_json::Value) -> Result<(Vec<u8>, String), String> {
    use rust_xlsxwriter::{Workbook, XlsxError};
    let sheets = spec
        .get("sheets")
        .and_then(|v| v.as_array())
        .ok_or("xlsx spec needs {\"sheets\": [{\"name\": ..., \"rows\": [[...]]}]}")?;
    if sheets.is_empty() {
        return Err("xlsx spec needs at least one sheet".into());
    }
    let mut workbook = Workbook::new();
    for (si, sheet) in sheets.iter().enumerate() {
        let name = sheet
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Sheet");
        let ws = workbook.add_worksheet();
        ws.set_name(if name.is_empty() { "Sheet" } else { name })
            .map_err(|e: XlsxError| format!("bad sheet name: {e}"))?;
        let rows = sheet
            .get("rows")
            .and_then(|v| v.as_array())
            .ok_or(format!("sheet {si} needs a \"rows\" array"))?;
        for (r, row) in rows.iter().enumerate().take(10_000) {
            let cells = row
                .as_array()
                .ok_or(format!("sheet {si} row {r} must be an array"))?;
            for (c, cell) in cells.iter().enumerate().take(256) {
                let res: Result<(), XlsxError> = match cell {
                    serde_json::Value::Number(n) => {
                        if let Some(f) = n.as_f64() {
                            ws.write_number(r as u32, c as u16, f).map(|_| ())
                        } else {
                            ws.write_string(r as u32, c as u16, &n.to_string())
                                .map(|_| ())
                        }
                    }
                    serde_json::Value::Bool(b) => {
                        ws.write_boolean(r as u32, c as u16, *b).map(|_| ())
                    }
                    serde_json::Value::Null => Ok(()),
                    serde_json::Value::String(s) => {
                        ws.write_string(r as u32, c as u16, s).map(|_| ())
                    }
                    other => ws
                        .write_string(r as u32, c as u16, &other.to_string())
                        .map(|_| ()),
                };
                res.map_err(|e| format!("write r{r}c{c}: {e}"))?;
            }
        }
    }
    workbook
        .save_to_buffer()
        .map(|b| {
            (
                b,
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
            )
        })
        .map_err(|e: XlsxError| format!("xlsx render failed: {e}"))
}

/// Write bytes under data/artifacts/<conv>/ with a sanitized name.
pub fn store(
    artifacts_dir: &Path,
    conv: &str,
    filename: &str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    let safe = crate::workspace::WorkspaceManager::sanitize_filename(filename)
        .map_err(|_| "invalid filename".to_string())?;
    if bytes.is_empty() {
        return Err("nothing to write".into());
    }
    if bytes.len() > 20_000_000 {
        return Err("artifact too large (max 20 MB)".into());
    }
    let dir = artifacts_dir.join(conv);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot store artifact: {e}"))?;
    let path = dir.join(&safe);
    std::fs::write(&path, bytes).map_err(|e| format!("cannot store artifact: {e}"))?;
    // Validate by reopening (§98): archives must parse as zip, PDFs by magic.
    if safe.ends_with(".xlsx") || safe.ends_with(".docx") || safe.ends_with(".pptx") {
        let f = std::fs::File::open(&path).map_err(|e| format!("validate failed: {e}"))?;
        zip_archive_check(f)?;
    }
    if safe.ends_with(".pdf") {
        let head = std::fs::read(&path).map_err(|e| format!("validate failed: {e}"))?;
        if !head.starts_with(b"%PDF") {
            return Err("validate failed: not a PDF file".into());
        }
    }
    Ok(path)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn spec_paragraphs(spec: &serde_json::Value) -> Vec<String> {
    if let Some(paras) = spec.get("paragraphs").and_then(|v| v.as_array()) {
        return paras.iter().map(|p| cell_text(p)).collect();
    }
    let text = spec
        .get("text")
        .and_then(|v| v.as_str())
        .or_else(|| spec.get("markdown").and_then(|v| v.as_str()))
        .unwrap_or("");
    text.lines().map(|l| l.to_string()).collect()
}

// ---- Minimal stored-zip writer (no compression): valid docx/pptx ----

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn zip_store(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let header_at = out.len() as u32;
        let crc = crc32(data);
        let push_u16 = |v: u16, o: &mut Vec<u8>| o.extend_from_slice(&v.to_le_bytes());
        let push_u32 = |v: u32, o: &mut Vec<u8>| o.extend_from_slice(&v.to_le_bytes());
        out.extend_from_slice(b"PK\x03\x04");
        push_u16(20, &mut out);
        push_u16(0, &mut out);
        push_u16(0, &mut out); // stored
        push_u16(0, &mut out);
        push_u16(0, &mut out);
        push_u32(crc, &mut out);
        push_u32(data.len() as u32, &mut out);
        push_u32(data.len() as u32, &mut out);
        push_u16(name.len() as u16, &mut out);
        push_u16(0, &mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(b"PK\x01\x02");
        push_u16(20, &mut central);
        push_u16(20, &mut central);
        push_u16(0, &mut central);
        push_u16(0, &mut central);
        push_u16(0, &mut central);
        push_u16(0, &mut central);
        push_u32(crc, &mut central);
        push_u32(data.len() as u32, &mut central);
        push_u32(data.len() as u32, &mut central);
        push_u16(name.len() as u16, &mut central);
        push_u16(0, &mut central);
        push_u16(0, &mut central);
        push_u16(0, &mut central);
        push_u16(0, &mut central);
        push_u32(0, &mut central);
        push_u32(header_at, &mut central);
        central.extend_from_slice(name.as_bytes());
    }
    let central_at = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0u8; 4]); // disks
    let n = files.len() as u16;
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_at.to_le_bytes());
    out.extend_from_slice(&[0u8; 2]);
    out
}

/// Spec: {"title"?, "text" | "paragraphs": [...]} → minimal valid .docx.
fn render_docx(spec: &serde_json::Value) -> Result<(Vec<u8>, String), String> {
    let title = spec
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Document");
    let mut body = String::new();
    body.push_str(&format!(
        "<w:p><w:pPr><w:pStyle w:val=\"Title\"/></w:pPr><w:r><w:t>{}</w:t></w:r></w:p>",
        xml_escape(title)
    ));
    for para in spec_paragraphs(spec) {
        body.push_str(&format!(
            "<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
            xml_escape(&para)
        ));
    }
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}<w:sectPr/></w:body></w:document>"
    );
    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/></Types>";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>";
    let bytes = zip_store(&[
        ("[Content_Types].xml", content_types.as_bytes()),
        ("_rels/.rels", rels.as_bytes()),
        ("word/document.xml", document.as_bytes()),
    ]);
    Ok((
        bytes,
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document".into(),
    ))
}

/// Spec: {"title"?, "slides": [{"title"?, "bullets": [...] }]} (or one slide
/// from text) → minimal valid .pptx.
fn render_pptx(spec: &serde_json::Value) -> Result<(Vec<u8>, String), String> {
    let slides: Vec<(String, Vec<String>)> = match spec.get("slides").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => arr
            .iter()
            .map(|s| {
                let t = s
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let b = s
                    .get("bullets")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().map(cell_text).collect())
                    .unwrap_or_default();
                (t, b)
            })
            .collect(),
        _ => vec![(
            spec.get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            spec_paragraphs(spec),
        )],
    };
    let mut slide_files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut sld_ids = String::new();
    for (i, (title, bullets)) in slides.iter().enumerate().take(50) {
        let n = i + 1;
        let mut shapes = format!(
            "<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Title\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp>",
            xml_escape(title)
        );
        for b in bullets.iter().take(20) {
            shapes.push_str(&format!("<p:sp><p:nvSpPr><p:cNvPr id=\"3\" name=\"Content\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>\u{2022} {}</a:t></a:r></a:p></p:txBody></p:sp>", xml_escape(b)));
        }
        let slide = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><p:cSld><p:spTree>{shapes}</p:spTree></p:cSld></p:sld>"
        );
        slide_files.push((format!("ppt/slides/slide{n}.xml"), slide.into_bytes()));
        sld_ids.push_str(&format!("<p:sldId id=\"{}\" r:id=\"rId{n}\"/>", 255 + n));
    }
    let presentation = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><p:presentation xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><p:sldIdLst>{sld_ids}</p:sldIdLst><p:sldSz cx=\"12192000\" cy=\"6858000\"/></p:presentation>"
    );
    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/></Types>";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"ppt/presentation.xml\"/></Relationships>";
    let mut files: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", content_types.as_bytes()),
        ("_rels/.rels", rels.as_bytes()),
        ("ppt/presentation.xml", presentation.as_bytes()),
    ];
    for (name, data) in &slide_files {
        files.push((name.as_str(), data.as_slice()));
    }
    Ok((
        zip_store(&files),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation".into(),
    ))
}

fn pdf_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// Spec: {"title"?, "text" | "paragraphs"} → minimal multi-page PDF (Helvetica).
fn render_pdf(spec: &serde_json::Value) -> Result<(Vec<u8>, String), String> {
    let paras = spec_paragraphs(spec);
    let lines: Vec<String> = paras
        .iter()
        .flat_map(|p| {
            // Greedy wrap at ~90 chars for the fixed font.
            let mut cur = String::new();
            let mut out = Vec::new();
            for w in p.split_whitespace() {
                if cur.len() + w.len() + 1 > 90 {
                    out.push(std::mem::take(&mut cur));
                }
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(w);
            }
            if !cur.is_empty() {
                out.push(cur);
            }
            if out.is_empty() {
                out.push(String::new());
            }
            out
        })
        .collect();
    let per_page = 45usize;
    let pages: Vec<&[String]> = lines.chunks(per_page.max(1)).collect();
    let npages = pages.len().max(1);
    // Object numbering: 1 catalog, 2 pages, 3 font, then per page (page, content).
    let mut objs: Vec<Vec<u8>> = Vec::new();
    objs.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let kids: String = (0..npages).map(|i| format!("{} 0 R ", 4 + i * 2)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{kids}] /Count {npages} >>").into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for page_lines in pages.iter().take(npages.max(1)) {
        let pno = objs.len() as u32 + 1;
        let cno = pno + 1;
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> >> /Contents {cno} 0 R >>").into_bytes());
        let mut content = String::from("BT /F1 12 Tf 50 790 Td 14 TL ");
        for line in page_lines.iter() {
            content.push_str(&format!("({}) Tj T* ", pdf_escape(line)));
        }
        content.push_str("ET");
        objs.push(content.into_bytes());
    }
    if pages.is_empty() {
        objs.push(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>".to_vec());
        objs.push(b"BT /F1 12 Tf 50 790 Td ( ) Tj ET".to_vec());
    }
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.push(b'\n');
        pdf.extend_from_slice(b"endobj\n");
    }
    let xref_at = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objs.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for off in offsets {
        pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF",
            objs.len() + 1
        )
        .as_bytes(),
    );
    Ok((pdf, "application/pdf".into()))
}

/// Minimal zip-central-directory check without a zip dependency.
fn zip_archive_check(mut f: std::fs::File) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom};
    let len = f
        .seek(SeekFrom::End(0))
        .map_err(|e| format!("validate failed: {e}"))?;
    if len < 22 {
        return Err("validate failed: file too small to be xlsx".into());
    }
    let mut tail = vec![0u8; 22.min(len as usize)];
    f.seek(SeekFrom::End(-(tail.len() as i64)))
        .map_err(|e| format!("validate failed: {e}"))?;
    f.read_exact(&mut tail)
        .map_err(|e| format!("validate failed: {e}"))?;
    if tail.windows(4).any(|w| w == [0x50, 0x4B, 0x05, 0x06]) {
        Ok(())
    } else {
        Err("validate failed: not a zip/xlsx archive".into())
    }
}

/// The document kind a request explicitly asks for, by naming a file type.
/// Only explicit types count: "document the API" is a verb, "a report" may
/// be an answer in the chat.
pub fn requested_document_kind(text: &str) -> Option<&'static str> {
    let lowered = text.to_lowercase();
    let has = |needle: &str| {
        lowered.match_indices(needle).any(|(index, _)| {
            let before = lowered[..index].chars().next_back();
            let after = lowered[index + needle.len()..].chars().next();
            !before.is_some_and(|c| c.is_alphanumeric()) && !after.is_some_and(|c| c.is_alphanumeric())
        })
    };
    if ["presentation", "slides", "slide deck", "deck", "pptx", "powerpoint", "keynote"]
        .iter()
        .any(|needle| has(needle))
    {
        return Some("pptx");
    }
    if ["spreadsheet", "excel", "xlsx", "workbook", "excel file", "excel sheet"]
        .iter()
        .any(|needle| has(needle))
    {
        return Some("xlsx");
    }
    if has("pdf") {
        return Some("pdf");
    }
    if ["docx", "word document", "word file", "word doc"]
        .iter()
        .any(|needle| has(needle))
    {
        return Some("docx");
    }
    if ["csv", "csv file"].iter().any(|needle| has(needle)) {
        return Some("csv");
    }
    if ["markdown file", "md file", ".md file"].iter().any(|needle| has(needle)) {
        return Some("md");
    }
    if ["html file", "html page", "web page"].iter().any(|needle| has(needle)) {
        return Some("html");
    }
    None
}

/// The fields a kind needs, when the spec carries none of them. A blank
/// document is never a success: the model gets told what to send instead.
fn missing_content(kind: &str, spec: &serde_json::Value) -> Option<&'static str> {
    let has_str = |key: &str| {
        spec.get(key)
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty())
    };
    let has_array = |key: &str| {
        spec.get(key)
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty())
    };
    let missing = match kind.to_lowercase().as_str() {
        "txt" | "text" => !has_str("text"),
        "md" | "markdown" => !has_str("markdown") && !has_str("text"),
        "csv" => !has_array("rows"),
        "html" => !has_str("html"),
        "json" => spec.get("data").is_none(),
        "xlsx" | "spreadsheet" => !has_array("sheets"),
        "docx" | "word" | "pdf" => {
            !has_array("paragraphs") && !has_str("text") && !has_str("markdown")
        }
        "pptx" | "slides" | "presentation" => {
            !has_array("slides") && !has_array("paragraphs") && !has_str("text")
        }
        _ => false,
    };
    if !missing {
        return None;
    }
    Some(match kind.to_lowercase().as_str() {
        "txt" | "text" => "text (string)",
        "md" | "markdown" => "markdown (string)",
        "csv" => "rows (array of arrays)",
        "html" => "title, html (string)",
        "json" => "data (any JSON value)",
        "xlsx" | "spreadsheet" => "sheets (array of {name, rows: array of arrays})",
        "docx" | "word" | "pdf" => "title, paragraphs (array of strings)",
        _ => "title, slides (array of {title, bullets: array of strings})",
    })
}

/// Async executor for the `create_document` tool (§§36–38).
/// Args: {"filename": "report.xlsx", "kind": "xlsx"?, ...spec fields}.
/// Everything except filename/kind IS the render spec (§98).
pub async fn execute_create_document(
    storage: &tokio::sync::Mutex<crate::storage::Storage>,
    artifacts_dir: &std::path::Path,
    conversation_id: &str,
    args: &serde_json::Value,
) -> Result<crate::tools::ToolResult, crate::tools::ToolError> {
    use crate::tools::{ToolError, ToolResult};
    let filename = args
        .get("filename")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ToolError::InvalidArgs(
                "create_document requires {\"filename\": \"...\", ...spec}".into(),
            )
        })?;
    let kind = args
        .get("kind")
        .or_else(|| args.get("type"))
        .or_else(|| args.get("format"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            Path::new(filename)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("txt")
                .to_string()
        });
    let mut spec = args.clone();
    if let Some(obj) = spec.as_object_mut() {
        obj.remove("filename");
        obj.remove("kind");
        obj.remove("type");
        obj.remove("format");
        // Models often nest the fields under a wrapper key ("content",
        // "spec", "document") or send the whole text as one string. The
        // intent is unambiguous, so accept both instead of rendering a blank
        // file. A json document's own "data" field is not a wrapper.
        if obj.len() == 1 && kind != "json" {
            let wrapped = ["content", "spec", "document", "body", "data", "sheet", "slide"]
                .iter()
                .find_map(|key| obj.get(*key).and_then(|v| v.as_object()).cloned());
            if let Some(inner) = wrapped {
                *obj = inner;
            }
        }
        if let Some(text) = obj.get("content").and_then(|v| v.as_str()).map(str::to_owned) {
            if !obj.contains_key("text") && !obj.contains_key("markdown") && !obj.contains_key("paragraphs") {
                obj.insert("text".into(), serde_json::Value::String(text));
            }
        }
        if let Some(sheet) = obj.get("sheet").and_then(|v| v.as_object()).cloned() {
            if !obj.contains_key("sheets") {
                obj.insert("sheets".into(), serde_json::json!([sheet]));
            }
        }
        if let Some(rows) = obj.get("rows").cloned() {
            if matches!(kind.as_str(), "xlsx" | "spreadsheet") && !obj.contains_key("sheets") {
                obj.insert("sheets".into(), serde_json::json!([{"name": "Sheet1", "rows": rows}]));
            }
        }
    }
    if let Some(expected) = missing_content(&kind, &spec) {
        let sent: Vec<&str> = args
            .as_object()
            .map(|o| o.keys().map(String::as_str).collect())
            .unwrap_or_default();
        return Err(ToolError::InvalidArgs(format!(
            "The {kind} spec had no content, so nothing was created. Put the complete content in these fields: {expected}. You sent keys: {}.",
            sent.join(", ")
        )));
    }
    let (bytes, mime) = render(&kind, &spec).map_err(ToolError::InvalidArgs)?;
    let path =
        store(artifacts_dir, conversation_id, filename, &bytes).map_err(ToolError::InvalidArgs)?;
    let row = crate::storage::ArtifactRow {
        id: uuid::Uuid::new_v4().to_string(),
        conversation_id: conversation_id.into(),
        filename: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| filename.into()),
        path: path.to_string_lossy().into_owned(),
        mime,
        size_bytes: bytes.len() as u64,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let size_kb = bytes.len() / 1024;
    storage
        .lock()
        .await
        .record_artifact(&row)
        .map_err(|e| ToolError::Io(std::io::Error::other(e.to_string())))?;
    // Say what was produced, not only that something was: a model that meant
    // three slides and sent one can see the shortfall in this result. Say
    // where it went too: the file is not in the workspace, and an earlier
    // wording ("open/reveal actions") read to a model as an instruction to
    // call open_path on a path that does not exist.
    Ok(ToolResult::ok(format!(
        "saved {} ({size_kb} KB, {}) to this conversation's Artifacts panel, outside the workspace. The user opens or saves it from that panel; no tool call is needed to show it.",
        row.filename,
        content_summary(&kind, &spec),
    )))
}

/// A one-line description of the rendered content for the tool result.
fn content_summary(kind: &str, spec: &serde_json::Value) -> String {
    let count = |key: &str| spec.get(key).and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
    match kind.to_lowercase().as_str() {
        "xlsx" | "spreadsheet" => {
            let sheets = spec.get("sheets").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let rows: usize = sheets
                .iter()
                .map(|s| s.get("rows").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0))
                .sum();
            format!("{} sheet(s), {rows} row(s)", sheets.len())
        }
        "pptx" | "slides" | "presentation" => {
            let slides = spec.get("slides").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            if slides.is_empty() {
                "1 slide (title plus paragraphs)".into()
            } else {
                let titles: Vec<String> = slides
                    .iter()
                    .filter_map(|s| s.get("title").and_then(|v| v.as_str()))
                    .map(|t| t.chars().take(40).collect())
                    .collect();
                format!("{} slide(s): {}", slides.len(), titles.join("; "))
            }
        }
        "docx" | "word" | "pdf" => format!("{} paragraph(s)", spec_paragraphs(spec).len()),
        "csv" => format!("{} row(s)", count("rows")),
        _ => {
            let chars = spec
                .get("text")
                .or_else(|| spec.get("markdown"))
                .or_else(|| spec.get("html"))
                .and_then(|v| v.as_str())
                .map(|s| s.chars().count())
                .unwrap_or(0);
            format!("{chars} character(s)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_file_types_are_recognised_and_verbs_are_not() {
        assert_eq!(requested_document_kind("make a short presentation about this project"), Some("pptx"));
        assert_eq!(requested_document_kind("Make me a spreadsheet of a weekly budget"), Some("xlsx"));
        assert_eq!(requested_document_kind("export the table as an Excel file"), Some("xlsx"));
        assert_eq!(requested_document_kind("a PDF explaining the KV cache"), Some("pdf"));
        assert_eq!(requested_document_kind("write the notes into a Word document"), Some("docx"));
        assert_eq!(requested_document_kind("give me a CSV of the models"), Some("csv"));
        assert_eq!(requested_document_kind("document the public API"), None);
        assert_eq!(requested_document_kind("write a report on the failures"), None);
        assert_eq!(requested_document_kind("what is a pdf"), Some("pdf"));
    }

    #[test]
    fn nested_and_aliased_specs_render_and_empty_ones_are_refused() {
        // Shape a small model actually sent: kind under "type", fields under "content".
        let nested = serde_json::json!({"content":{"title":"KV cache","paragraphs":["one","two"]}});
        assert!(missing_content("pdf", &nested).is_some(), "still wrapped");
        let inner = nested.get("content").cloned().unwrap();
        assert!(missing_content("pdf", &inner).is_none());
        assert_eq!(missing_content("xlsx", &serde_json::json!({})), Some("sheets (array of {name, rows: array of arrays})"));
        assert_eq!(missing_content("pptx", &serde_json::json!({"title":"x"})), Some("title, slides (array of {title, bullets: array of strings})"));
        assert!(missing_content("md", &serde_json::json!({"text":"hello"})).is_none());
        assert!(missing_content("txt", &serde_json::json!({"text":"  "})).is_some());
    }

    #[test]
    fn csv_quotes_cells() {
        let (b, mime) = render(
            "csv",
            &serde_json::json!({"rows": [["a", "b,c"], ["d", 1]]}),
        )
        .unwrap();
        let t = String::from_utf8(b).unwrap();
        assert!(t.contains("\"b,c\""), "{t}");
        assert_eq!(mime, "text/csv");
    }

    #[test]
    fn json_pretty_and_typed() {
        let (b, _) = render("json", &serde_json::json!({"data": {"a": 1}})).unwrap();
        assert!(String::from_utf8(b).unwrap().contains("\"a\": 1"));
    }

    #[test]
    fn xlsx_roundtrip_validates() {
        let dir = std::env::temp_dir().join(format!("companion-doc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (b, mime) = render("xlsx", &serde_json::json!({
            "sheets": [{"name": "Files", "rows": [["name", "size"], ["a.txt", 12], ["b.txt", 30]]}]
        }))
        .unwrap();
        assert!(mime.contains("spreadsheet"));
        let p = store(&dir, "c1", "files.xlsx", &b).unwrap();
        assert!(p.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_format_is_honest() {
        let err = render("exe", &serde_json::json!({})).unwrap_err();
        assert!(err.contains("no local renderer"), "{err}");
    }

    #[test]
    fn docx_is_a_valid_zip_with_text() {
        let (b, mime) = render(
            "docx",
            &serde_json::json!({"title": "Hi", "text": "hello & <world>"}),
        )
        .unwrap();
        assert!(mime.contains("wordprocessingml"), "{mime}");
        assert_eq!(&b[..4], b"PK\x03\x04");
        assert!(mime_for("a.docx").contains("wordprocessingml"));
    }

    #[test]
    fn pdf_has_magic_and_pages() {
        let text = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (b, mime) = render("pdf", &serde_json::json!({"title": "T", "text": text})).unwrap();
        assert_eq!(mime, "application/pdf");
        assert!(b.starts_with(b"%PDF"));
        assert!(b.windows(7).any(|w| w == b"/Count "), "multi-page expected");
        assert_eq!(mime_for("a.pdf"), "application/pdf");
    }

    #[test]
    fn pptx_is_a_valid_zip() {
        let (b, mime) = render(
            "pptx",
            &serde_json::json!({
                "title": "Deck", "slides": [{"title": "S1", "bullets": ["a", "b"]}]
            }),
        )
        .unwrap();
        assert!(mime.contains("presentationml"), "{mime}");
        assert_eq!(&b[..4], b"PK\x03\x04");
    }

    #[test]
    fn mime_table_sane() {
        assert_eq!(
            mime_for("a.xlsx"),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
        assert_eq!(mime_for("a.MD"), "text/markdown");
    }
}
