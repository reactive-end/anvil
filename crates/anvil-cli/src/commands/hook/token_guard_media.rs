//! Binary/media file detection, PDF `pages` range parsing, and byte-size formatting for Token Guard.

use std::fs::File;
use std::io::Read;
use std::path::Path;

const SNIFF_BYTES: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FileKind {
    Pdf,
    Image,
    AudioVideo,
    OtherBinary,
    Text,
}

/// Classifies a file by lowercase extension first, falling back to magic bytes / NUL detection.
pub(super) fn classify_file_kind(path: &Path) -> FileKind {
    kind_from_extension(path).unwrap_or_else(|| kind_from_header(path))
}

fn kind_from_extension(path: &Path) -> Option<FileKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => Some(FileKind::Pdf),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff" | "avif"
        | "heic" | "heif" => Some(FileKind::Image),
        "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" | "mp4" | "mov" | "webm" | "mkv" | "avi" => {
            Some(FileKind::AudioVideo)
        }
        "exe" | "dll" | "so" | "dylib" | "zip" | "tar" | "gz" | "7z" | "rar" | "wasm"
        | "sqlite" | "db" | "bin" | "o" | "a" | "class" | "jar" | "pyc" | "docx" | "xlsx"
        | "pptx" => Some(FileKind::OtherBinary),
        _ => None,
    }
}

fn kind_from_header(path: &Path) -> FileKind {
    let Ok(mut file) = File::open(path) else {
        return FileKind::Text;
    };
    let mut buf = [0u8; SNIFF_BYTES];
    let Ok(n) = file.read(&mut buf) else {
        return FileKind::Text;
    };
    header_magic_kind(&buf[..n])
}

fn header_magic_kind(buf: &[u8]) -> FileKind {
    if buf.starts_with(b"%PDF-") {
        return FileKind::Pdf;
    }
    if is_image_magic(buf) {
        return FileKind::Image;
    }
    if buf.starts_with(b"\xFF\xFE") || buf.starts_with(b"\xFE\xFF") {
        return FileKind::Text;
    }
    if buf.contains(&0) {
        return FileKind::OtherBinary;
    }
    FileKind::Text
}

fn is_image_magic(buf: &[u8]) -> bool {
    let is_webp = buf.len() >= 12 && &buf[..4] == b"RIFF" && &buf[8..12] == b"WEBP";
    buf.starts_with(b"\x89PNG\r\n\x1a\n")
        || buf.starts_with(b"\xFF\xD8\xFF")
        || buf.starts_with(b"GIF87a")
        || buf.starts_with(b"GIF89a")
        || is_webp
        || buf.starts_with(b"\x00\x00\x01\x00")
        || buf.starts_with(b"II*\x00")
        || buf.starts_with(b"MM\x00*")
}

/// Parses Claude Code PDF page range specifications (`"1-5"`, `"1,3,5-8"`) into total page count.
pub(super) fn parse_pdf_page_count(spec: &str) -> Option<usize> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut total = 0usize;
    for part in trimmed.split(',') {
        let token = part.trim();
        if token.is_empty() {
            return None;
        }
        let span = parse_single_page_token(token)?;
        total = total.checked_add(span)?;
    }
    (total > 0).then_some(total)
}

fn parse_single_page_token(token: &str) -> Option<usize> {
    let Some((start_raw, end_raw)) = token.split_once('-') else {
        let page: usize = token.parse().ok()?;
        return (page >= 1).then_some(1);
    };
    let start: usize = start_raw.trim().parse().ok()?;
    let end: usize = end_raw.trim().parse().ok()?;
    if start == 0 || end < start {
        return None;
    }
    end.checked_sub(start)?.checked_add(1)
}

/// Formats byte counts cleanly for human-readable Token Guard messages.
pub(super) fn format_bytes_human(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const KB: u64 = 1024;
    if bytes >= MB {
        format!("{:.2} MB", (bytes as f64) / (MB as f64))
    } else if bytes >= KB {
        format!("{:.1} KB", (bytes as f64) / (KB as f64))
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parses_valid_and_rejects_invalid_pdf_page_specs() {
        assert_eq!(parse_pdf_page_count("1-2"), Some(2));
        assert_eq!(parse_pdf_page_count("5"), Some(1));
        assert_eq!(parse_pdf_page_count("1, 3-5, 10"), Some(5));
        assert_eq!(parse_pdf_page_count(""), None);
        assert_eq!(parse_pdf_page_count("0"), None);
        assert_eq!(parse_pdf_page_count("5-2"), None);
        assert_eq!(parse_pdf_page_count("1,,3"), None);
        assert_eq!(parse_pdf_page_count("1-abc"), None);
    }

    #[test]
    fn detects_magic_bytes_nul_binaries_and_plain_utf8_text() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();

        let extless_pdf = dir.join("attachment");
        fs::write(&extless_pdf, b"%PDF-1.7\n%\xE2\xE3\n").unwrap();
        assert_eq!(classify_file_kind(&extless_pdf), FileKind::Pdf);

        let extless_png = dir.join("screenshot");
        fs::write(&extless_png, b"\x89PNG\r\n\x1a\n\x00\x00").unwrap();
        assert_eq!(classify_file_kind(&extless_png), FileKind::Image);

        let bin = dir.join("blob.dat");
        fs::write(&bin, b"HEADER\x00PAYLOAD").unwrap();
        assert_eq!(classify_file_kind(&bin), FileKind::OtherBinary);

        let txt = dir.join("notes.txt");
        fs::write(&txt, "Linea 1\nLinea 2 con tildes \u{00e1}\n").unwrap();
        assert_eq!(classify_file_kind(&txt), FileKind::Text);

        let utf16 = dir.join("notes-utf16.txt");
        fs::write(&utf16, b"\xFF\xFEH\x00i\x00\n\x00").unwrap();
        assert_eq!(classify_file_kind(&utf16), FileKind::Text);
    }

    #[test]
    fn formats_byte_sizes_human_readably() {
        assert_eq!(format_bytes_human(500), "500 B");
        assert_eq!(format_bytes_human(2048), "2.0 KB");
        assert_eq!(format_bytes_human(5 * 1024 * 1024), "5.00 MB");
    }
}
