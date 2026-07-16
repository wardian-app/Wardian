use super::authorized_roots::FileIdentity;
use super::AuthorizedPath;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::time::UNIX_EPOCH;

const SCAN_BUFFER_SIZE: usize = 64 * 1024;
const SIGNATURE_PROBE_SIZE: usize = 1024 * 1024;
const MINIMUM_DETECTION_BYTES: u64 = 32;
const DESCRIPTOR_SCAN_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Versioned metadata, content classification, and safe renderer capabilities
/// for one canonical file revision.
pub struct FileContentDescriptorV1 {
    /// Descriptor schema version. This type always emits version 1.
    pub schema: u8,
    /// Canonical filesystem path used as resource identity.
    pub canonical_path: String,
    /// Final path component suitable for display.
    pub display_name: String,
    /// Lowercase filename extension hint, when present.
    pub extension: Option<String>,
    /// MIME type confirmed from a signature or validated text content.
    pub mime_type: String,
    /// Validated text encoding, or `None` for non-text content.
    pub encoding: Option<String>,
    /// Backend-confirmed renderer family.
    pub renderer_kind: FileRendererKind,
    /// Exact byte count of the content that was hashed.
    pub size_bytes: u64,
    /// Validated UTF-8 line count, or `None` for non-text content.
    pub line_count: Option<u64>,
    /// SHA-256 content identity prefixed with `sha256:`.
    pub content_hash: String,
    /// Last modification time as Unix epoch milliseconds.
    pub modified_at_ms: u64,
    /// Operations permitted for this content under the active limits.
    pub capabilities: FileResourceCapabilitiesV1,
    /// Stable machine-readable reason that preview is unavailable.
    pub unavailable_reason: Option<String>,
}

impl FileContentDescriptorV1 {
    /// Detects content from a previously authorized canonical file. Signature
    /// checks take precedence over extension hints, and text requires valid
    /// UTF-8 without binary control bytes.
    ///
    /// # Errors
    ///
    /// Returns `unauthorized_path` if the original path no longer resolves to
    /// the authorized handle, `unstable_file` if no stable revision can be
    /// scanned after retrying, `file_too_large` if the detected renderer's
    /// centralized byte ceiling is exceeded, or `unavailable_path` for other
    /// file access and metadata failures.
    pub fn from_authorized_path(
        authorized: &AuthorizedPath,
        limits: &FileResourceLimits,
    ) -> Result<Self, FileResourceErrorV1> {
        Self::from_authorized_path_with_hook(authorized, limits, |_, _| {})
    }

    fn from_authorized_path_with_hook(
        authorized: &AuthorizedPath,
        limits: &FileResourceLimits,
        mut after_read: impl FnMut(usize, u64),
    ) -> Result<Self, FileResourceErrorV1> {
        let mut file = authorized.lock_verified_file()?;
        let canonical_path = authorized
            .verified_canonical_path()
            .to_str()
            .ok_or_else(|| {
                FileResourceErrorV1::new(
                    "unavailable_path",
                    "canonical path cannot be represented losslessly as UTF-8",
                )
            })?;
        let display_name = authorized
            .verified_canonical_path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(canonical_path)
            .to_string();
        let extension = authorized
            .verified_canonical_path()
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);
        for attempt in 0..DESCRIPTOR_SCAN_ATTEMPTS {
            authorized.verify_current_binding(&file)?;
            let before = revision_metadata(&file)?;
            file.seek(SeekFrom::Start(0)).map_err(|error| {
                FileResourceErrorV1::new("unavailable_path", format!("cannot seek file: {error}"))
            })?;
            let scan = scan_reader_limited(
                &mut BufReader::new(&mut *file),
                SIGNATURE_PROBE_SIZE,
                limits,
                |bytes_read| after_read(attempt, bytes_read),
            )?;
            let after = revision_metadata(&file)?;
            authorized.verify_current_binding(&file)?;
            if before != after || scan.size_bytes != after.size_bytes {
                continue;
            }

            return descriptor_from_scan(
                canonical_path,
                display_name.clone(),
                extension.clone(),
                scan,
                after.modified_at_ms,
                limits,
            );
        }

        Err(FileResourceErrorV1::new(
            "unstable_file",
            "file changed during every descriptor scan attempt",
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileRevision {
    identity: FileIdentity,
    size_bytes: u64,
    modified_at_ms: u64,
    write_marker: i128,
    change_marker: i128,
}

fn revision_metadata(file: &std::fs::File) -> Result<FileRevision, FileResourceErrorV1> {
    let metadata = file.metadata().map_err(|error| {
        FileResourceErrorV1::new(
            "unavailable_path",
            format!("cannot read file metadata: {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(FileResourceErrorV1::new(
            "unavailable_path",
            "authorized handle is not a file",
        ));
    }
    let identity = FileIdentity::from_file(file).map_err(|error| {
        FileResourceErrorV1::new(
            "unavailable_path",
            format!("cannot read file identity: {error}"),
        )
    })?;
    let modified_at_ms = metadata
        .modified()
        .and_then(|modified| {
            modified
                .duration_since(UNIX_EPOCH)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
        })
        .map_err(|error| {
            FileResourceErrorV1::new(
                "unavailable_path",
                format!("cannot resolve modification time: {error}"),
            )
        })?
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);

    let (write_marker, change_marker) = metadata_revision_markers(&metadata);
    Ok(FileRevision {
        identity,
        size_bytes: metadata.len(),
        modified_at_ms,
        write_marker,
        change_marker,
    })
}

#[cfg(unix)]
fn metadata_revision_markers(metadata: &std::fs::Metadata) -> (i128, i128) {
    use std::os::unix::fs::MetadataExt;

    (
        (i128::from(metadata.mtime()) * 1_000_000_000) + i128::from(metadata.mtime_nsec()),
        (i128::from(metadata.ctime()) * 1_000_000_000) + i128::from(metadata.ctime_nsec()),
    )
}

#[cfg(windows)]
fn metadata_revision_markers(metadata: &std::fs::Metadata) -> (i128, i128) {
    use std::os::windows::fs::MetadataExt;

    (i128::from(metadata.last_write_time()), 0)
}

#[cfg(not(any(unix, windows)))]
fn metadata_revision_markers(metadata: &std::fs::Metadata) -> (i128, i128) {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i128)
        .unwrap_or_default();
    (modified, 0)
}

fn descriptor_from_scan(
    canonical_path: &str,
    display_name: String,
    extension: Option<String>,
    scan: ScannedFile,
    modified_at_ms: u64,
    limits: &FileResourceLimits,
) -> Result<FileContentDescriptorV1, FileResourceErrorV1> {
    let detected = detect_content(
        &scan.probe,
        extension.as_deref(),
        scan.is_utf8_text,
        scan.line_count,
    );
    let (capabilities, unavailable_reason) = capabilities_for(
        detected.renderer_kind,
        scan.size_bytes,
        detected.line_count,
        detected.image_pixels,
        limits,
    );

    Ok(FileContentDescriptorV1 {
        schema: 1,
        canonical_path: canonical_path.to_string(),
        display_name,
        extension,
        mime_type: detected.mime_type.to_string(),
        encoding: detected.line_count.map(|_| "utf-8".to_string()),
        renderer_kind: detected.renderer_kind,
        size_bytes: scan.size_bytes,
        line_count: detected.line_count,
        content_hash: scan.content_hash,
        modified_at_ms,
        capabilities,
        unavailable_reason,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Backend-confirmed renderer family selected from signatures and validated
/// content rather than from a filename extension alone.
pub enum FileRendererKind {
    /// Validated UTF-8 text or source code.
    Text,
    /// Validated UTF-8 Markdown.
    Markdown,
    /// Signature-confirmed PNG, JPEG, GIF, or WebP.
    Image,
    /// Signature-confirmed PDF.
    Pdf,
    /// Invalid UTF-8 or binary content without a supported signature.
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Operations that are safe for the detected content under the active limits.
pub struct FileResourceCapabilitiesV1 {
    /// Whether the content can be safely previewed.
    pub preview: bool,
    /// Whether each text side fits the centralized diff limits.
    pub changes: bool,
    /// Whether the content can back a complete editable text model.
    pub draft: bool,
    /// Whether the content can be served through a bounded byte stream.
    pub stream: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Central byte, line, and decoded-pixel limits shared by Files consumers.
pub struct FileResourceLimits {
    /// Maximum complete text-model size.
    pub monaco_max_size_bytes: u64,
    /// Maximum complete text-model line count.
    pub monaco_max_line_count: u64,
    /// Maximum bytes on either side of a text diff.
    pub diff_max_size_bytes_per_side: u64,
    /// Maximum lines on either side of a text diff.
    pub diff_max_line_count: u64,
    /// Maximum encoded image size.
    pub image_max_size_bytes: u64,
    /// Maximum decoded image pixel count.
    pub image_max_pixels: u64,
    /// Maximum streamed PDF size.
    pub pdf_max_size_bytes: u64,
}

impl FileResourceLimits {
    /// Returns whether a text model fits the Monaco byte and line limits.
    #[must_use]
    pub fn allows_monaco(&self, size_bytes: u64, line_count: u64) -> bool {
        size_bytes <= self.monaco_max_size_bytes && line_count <= self.monaco_max_line_count
    }

    /// Returns whether one side of a future text diff fits its limits.
    #[must_use]
    pub fn allows_diff_side(&self, size_bytes: u64, line_count: u64) -> bool {
        size_bytes <= self.diff_max_size_bytes_per_side && line_count <= self.diff_max_line_count
    }

    /// Returns whether encoded and decoded image sizes fit their limits.
    #[must_use]
    pub fn allows_image(&self, size_bytes: u64, pixels: u64) -> bool {
        size_bytes <= self.image_max_size_bytes && pixels <= self.image_max_pixels
    }

    /// Returns whether a PDF fits its streaming byte limit.
    #[must_use]
    pub fn allows_pdf(&self, size_bytes: u64) -> bool {
        size_bytes <= self.pdf_max_size_bytes
    }
}

impl Default for FileResourceLimits {
    fn default() -> Self {
        Self {
            monaco_max_size_bytes: 16 * 1024 * 1024,
            monaco_max_line_count: 200_000,
            diff_max_size_bytes_per_side: 5 * 1024 * 1024,
            diff_max_line_count: 100_000,
            image_max_size_bytes: 64 * 1024 * 1024,
            image_max_pixels: 64_000_000,
            pdf_max_size_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
#[serde(rename_all = "snake_case")]
/// Resource-local, serializable Files error returned across service boundaries.
pub struct FileResourceErrorV1 {
    /// Error schema version. This type always emits version 1.
    pub schema: u8,
    /// Stable machine-readable error category.
    pub code: String,
    /// Resource-local diagnostic text suitable for logs and UI fallback.
    pub message: String,
}

impl FileResourceErrorV1 {
    /// Creates a schema-v1 error with a stable machine-readable code.
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            schema: 1,
            code: code.into(),
            message: message.into(),
        }
    }

    /// Returns the stable machine-readable error code.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }
}

struct DetectedContent {
    mime_type: &'static str,
    renderer_kind: FileRendererKind,
    line_count: Option<u64>,
    image_pixels: Option<u64>,
}

fn detect_content(
    bytes: &[u8],
    extension: Option<&str>,
    is_utf8_text: bool,
    line_count: Option<u64>,
) -> DetectedContent {
    let signature = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", FileRendererKind::Image))
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("image/jpeg", FileRendererKind::Image))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("image/gif", FileRendererKind::Image))
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(("image/webp", FileRendererKind::Image))
    } else if bytes.starts_with(b"%PDF-") {
        Some(("application/pdf", FileRendererKind::Pdf))
    } else {
        None
    };

    if let Some((mime_type, renderer_kind)) = signature {
        return DetectedContent {
            mime_type,
            renderer_kind,
            line_count: None,
            image_pixels: image_pixels(bytes, mime_type),
        };
    }

    if is_utf8_text {
        let (mime_type, renderer_kind) = text_kind(extension);
        return DetectedContent {
            mime_type,
            renderer_kind,
            line_count,
            image_pixels: None,
        };
    }

    DetectedContent {
        mime_type: "application/octet-stream",
        renderer_kind: FileRendererKind::Unsupported,
        line_count: None,
        image_pixels: None,
    }
}

#[derive(Debug)]
struct ScannedFile {
    probe: Vec<u8>,
    size_bytes: u64,
    content_hash: String,
    is_utf8_text: bool,
    line_count: Option<u64>,
}

#[cfg(test)]
fn scan_reader(reader: &mut impl Read, probe_limit: usize) -> std::io::Result<ScannedFile> {
    scan_reader_with_hook(reader, probe_limit, |_| {})
}

#[cfg(test)]
fn scan_reader_with_hook(
    reader: &mut impl Read,
    probe_limit: usize,
    mut after_read: impl FnMut(u64),
) -> std::io::Result<ScannedFile> {
    let mut buffer = [0_u8; SCAN_BUFFER_SIZE];
    let mut probe = Vec::with_capacity(probe_limit.min(SCAN_BUFFER_SIZE));
    let mut size_bytes = 0_u64;
    let mut hasher = Sha256::new();
    let mut utf8 = Utf8TextAnalyzer::default();

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        let chunk = &buffer[..bytes_read];
        size_bytes = size_bytes.saturating_add(bytes_read.try_into().unwrap_or(u64::MAX));
        after_read(size_bytes);
        hasher.update(chunk);
        if probe.len() < probe_limit {
            let remaining = probe_limit - probe.len();
            probe.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        }
        utf8.push(chunk);
    }

    let (is_utf8_text, line_count) = utf8.finish();
    let digest = hasher.finalize();
    Ok(ScannedFile {
        probe,
        size_bytes,
        content_hash: format!("sha256:{digest:x}"),
        is_utf8_text,
        line_count,
    })
}

fn scan_reader_limited(
    reader: &mut impl Read,
    probe_limit: usize,
    limits: &FileResourceLimits,
    mut after_read: impl FnMut(u64),
) -> Result<ScannedFile, FileResourceErrorV1> {
    let mut buffer = [0_u8; SCAN_BUFFER_SIZE];
    let mut probe = Vec::with_capacity(probe_limit.min(SCAN_BUFFER_SIZE));
    let mut size_bytes = 0_u64;
    let mut scan_ceiling = None;
    let mut hasher = Sha256::new();
    let mut utf8 = Utf8TextAnalyzer::default();

    loop {
        let allowed_total = scan_ceiling
            .map(|ceiling: u64| ceiling.saturating_add(1))
            .unwrap_or(MINIMUM_DETECTION_BYTES);
        let remaining = allowed_total.saturating_sub(size_bytes);
        if remaining == 0 {
            if let Some(ceiling) = scan_ceiling {
                return Err(oversized_scan(ceiling));
            }
        }
        let request_size = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let bytes_read = reader.read(&mut buffer[..request_size]).map_err(|error| {
            FileResourceErrorV1::new("unavailable_path", format!("cannot scan file: {error}"))
        })?;
        if bytes_read == 0 {
            break;
        }
        let chunk = &buffer[..bytes_read];
        size_bytes = size_bytes.saturating_add(bytes_read.try_into().unwrap_or(u64::MAX));
        after_read(size_bytes);
        hasher.update(chunk);
        if probe.len() < probe_limit {
            let remaining = probe_limit - probe.len();
            probe.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        }
        utf8.push(chunk);

        if scan_ceiling.is_none() && size_bytes >= MINIMUM_DETECTION_BYTES {
            scan_ceiling = Some(content_scan_ceiling(&probe, limits));
        }
        if scan_ceiling.is_some_and(|ceiling| size_bytes > ceiling) {
            return Err(oversized_scan(scan_ceiling.unwrap_or_default()));
        }
    }

    let ceiling = scan_ceiling.unwrap_or_else(|| content_scan_ceiling(&probe, limits));
    if size_bytes > ceiling {
        return Err(oversized_scan(ceiling));
    }
    let (is_utf8_text, line_count) = utf8.finish();
    let digest = hasher.finalize();
    Ok(ScannedFile {
        probe,
        size_bytes,
        content_hash: format!("sha256:{digest:x}"),
        is_utf8_text,
        line_count,
    })
}

fn content_scan_ceiling(probe: &[u8], limits: &FileResourceLimits) -> u64 {
    if probe.starts_with(b"\x89PNG\r\n\x1a\n")
        || probe.starts_with(&[0xff, 0xd8, 0xff])
        || probe.starts_with(b"GIF87a")
        || probe.starts_with(b"GIF89a")
        || (probe.starts_with(b"RIFF") && probe.get(8..12) == Some(b"WEBP"))
    {
        limits.image_max_size_bytes
    } else if probe.starts_with(b"%PDF-") {
        limits.pdf_max_size_bytes
    } else {
        limits.monaco_max_size_bytes
    }
}

fn oversized_scan(ceiling: u64) -> FileResourceErrorV1 {
    FileResourceErrorV1::new(
        "file_too_large",
        format!("file exceeds the detected renderer scan ceiling of {ceiling} bytes"),
    )
}

#[derive(Default)]
struct Utf8TextAnalyzer {
    carry: Vec<u8>,
    is_valid: bool,
    looks_like_text: bool,
    line_count: u64,
    previous_was_cr: bool,
    initialized: bool,
}

impl Utf8TextAnalyzer {
    fn push(&mut self, chunk: &[u8]) {
        if !self.initialized {
            self.is_valid = true;
            self.looks_like_text = true;
            self.line_count = 1;
            self.initialized = true;
        }
        if !self.is_valid {
            return;
        }

        let mut combined = Vec::with_capacity(self.carry.len() + chunk.len());
        combined.extend_from_slice(&self.carry);
        combined.extend_from_slice(chunk);
        self.carry.clear();

        match std::str::from_utf8(&combined) {
            Ok(text) => self.push_valid_text(text),
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                let valid_prefix = std::str::from_utf8(&combined[..valid_up_to])
                    .expect("UTF-8 error valid prefix must be valid");
                self.push_valid_text(valid_prefix);
                if error.error_len().is_some() {
                    self.is_valid = false;
                } else {
                    self.carry.extend_from_slice(&combined[valid_up_to..]);
                }
            }
        }
    }

    fn push_valid_text(&mut self, text: &str) {
        for character in text.chars() {
            if self.previous_was_cr {
                self.previous_was_cr = false;
                if character == '\n' {
                    continue;
                }
            }
            match character {
                '\r' => {
                    self.line_count = self.line_count.saturating_add(1);
                    self.previous_was_cr = true;
                }
                '\n' => self.line_count = self.line_count.saturating_add(1),
                '\t' => {}
                '\0' => self.looks_like_text = false,
                character if character.is_control() => self.looks_like_text = false,
                _ => {}
            }
        }
    }

    fn finish(mut self) -> (bool, Option<u64>) {
        if !self.initialized {
            self.is_valid = true;
            self.looks_like_text = true;
            self.line_count = 1;
        }
        if !self.carry.is_empty() {
            self.is_valid = false;
        }
        let is_utf8_text = self.is_valid && self.looks_like_text;
        (is_utf8_text, is_utf8_text.then_some(self.line_count))
    }
}

fn text_kind(extension: Option<&str>) -> (&'static str, FileRendererKind) {
    match extension {
        Some("md" | "markdown") => ("text/markdown", FileRendererKind::Markdown),
        Some("html" | "htm") => ("text/html", FileRendererKind::Text),
        Some("svg") => ("image/svg+xml", FileRendererKind::Text),
        Some("json") => ("application/json", FileRendererKind::Text),
        Some("yaml" | "yml") => ("application/yaml", FileRendererKind::Text),
        Some("toml") => ("application/toml", FileRendererKind::Text),
        _ => ("text/plain", FileRendererKind::Text),
    }
}

fn capabilities_for(
    renderer_kind: FileRendererKind,
    size_bytes: u64,
    line_count: Option<u64>,
    image_pixels: Option<u64>,
    limits: &FileResourceLimits,
) -> (FileResourceCapabilitiesV1, Option<String>) {
    match renderer_kind {
        FileRendererKind::Text | FileRendererKind::Markdown => {
            let line_count = line_count.unwrap_or(u64::MAX);
            let preview = limits.allows_monaco(size_bytes, line_count);
            let changes = limits.allows_diff_side(size_bytes, line_count);
            let unavailable_reason = if size_bytes > limits.monaco_max_size_bytes {
                Some("monaco_size_limit_exceeded".to_string())
            } else if line_count > limits.monaco_max_line_count {
                Some("monaco_line_limit_exceeded".to_string())
            } else {
                None
            };
            (
                FileResourceCapabilitiesV1 {
                    preview,
                    changes,
                    draft: preview,
                    stream: false,
                },
                unavailable_reason,
            )
        }
        FileRendererKind::Image => {
            let preview =
                image_pixels.is_some_and(|pixels| limits.allows_image(size_bytes, pixels));
            let unavailable_reason = if image_pixels.is_none() {
                Some("image_dimensions_unavailable".to_string())
            } else if !preview {
                Some("image_limit_exceeded".to_string())
            } else {
                None
            };
            (
                FileResourceCapabilitiesV1 {
                    preview,
                    changes: false,
                    draft: false,
                    stream: preview,
                },
                unavailable_reason,
            )
        }
        FileRendererKind::Pdf => {
            let preview = limits.allows_pdf(size_bytes);
            (
                FileResourceCapabilitiesV1 {
                    preview,
                    changes: false,
                    draft: false,
                    stream: preview,
                },
                (!preview).then(|| "pdf_size_limit_exceeded".to_string()),
            )
        }
        FileRendererKind::Unsupported => (
            FileResourceCapabilitiesV1 {
                preview: false,
                changes: false,
                draft: false,
                stream: false,
            },
            Some("unsupported_content".to_string()),
        ),
    }
}

fn image_pixels(bytes: &[u8], mime_type: &str) -> Option<u64> {
    let (width, height) = match mime_type {
        "image/png" if bytes.len() >= 24 => (
            u32::from_be_bytes(bytes[16..20].try_into().ok()?) as u64,
            u32::from_be_bytes(bytes[20..24].try_into().ok()?) as u64,
        ),
        "image/gif" if bytes.len() >= 10 => (
            u16::from_le_bytes(bytes[6..8].try_into().ok()?) as u64,
            u16::from_le_bytes(bytes[8..10].try_into().ok()?) as u64,
        ),
        "image/jpeg" => jpeg_dimensions(bytes)?,
        "image/webp" => webp_dimensions(bytes)?,
        _ => return None,
    };
    width.checked_mul(height)
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u64, u64)> {
    let mut offset = 2;
    while offset + 3 < bytes.len() {
        if bytes[offset] != 0xff {
            offset += 1;
            continue;
        }
        while offset < bytes.len() && bytes[offset] == 0xff {
            offset += 1;
        }
        let marker = *bytes.get(offset)?;
        offset += 1;
        if matches!(marker, 0xd8 | 0xd9 | 0x01) || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let segment_length =
            u16::from_be_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?) as usize;
        if segment_length < 2 || offset.checked_add(segment_length)? > bytes.len() {
            return None;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let height =
                u16::from_be_bytes(bytes.get(offset + 3..offset + 5)?.try_into().ok()?) as u64;
            let width =
                u16::from_be_bytes(bytes.get(offset + 5..offset + 7)?.try_into().ok()?) as u64;
            return Some((width, height));
        }
        offset += segment_length;
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u64, u64)> {
    let chunk = bytes.get(12..16)?;
    if chunk == b"VP8X" {
        let width = 1 + read_u24_le(bytes.get(24..27)?)?;
        let height = 1 + read_u24_le(bytes.get(27..30)?)?;
        return Some((width, height));
    }
    if chunk == b"VP8 " && bytes.get(23..26)? == [0x9d, 0x01, 0x2a] {
        let width = u16::from_le_bytes(bytes.get(26..28)?.try_into().ok()?) & 0x3fff;
        let height = u16::from_le_bytes(bytes.get(28..30)?.try_into().ok()?) & 0x3fff;
        return Some((width as u64, height as u64));
    }
    if chunk == b"VP8L" && *bytes.get(20)? == 0x2f {
        let packed = u32::from_le_bytes(bytes.get(21..25)?.try_into().ok()?);
        let width = 1 + (packed & 0x3fff) as u64;
        let height = 1 + ((packed >> 14) & 0x3fff) as u64;
        return Some((width, height));
    }
    None
}

fn read_u24_le(bytes: &[u8]) -> Option<u64> {
    Some(
        bytes.first().copied()? as u64
            | ((bytes.get(1).copied()? as u64) << 8)
            | ((bytes.get(2).copied()? as u64) << 16),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::AuthorizedRootService;
    use crate::models::AgentConfig;
    use serde_json::json;
    use std::fs;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::Path;

    fn describe(name: &str, bytes: &[u8]) -> FileContentDescriptorV1 {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join(name);
        fs::write(&path, bytes).expect("fixture file");
        let service = AuthorizedRootService::from_agent_config(&AgentConfig {
            folder: temp.path().to_string_lossy().into_owned(),
            ..AgentConfig::default()
        })
        .expect("valid root");
        let authorized = service
            .authorize_existing_file(&path)
            .expect("authorized fixture");
        FileContentDescriptorV1::from_authorized_path(&authorized, &FileResourceLimits::default())
            .expect("descriptor")
    }

    #[test]
    fn descriptor_rejects_ancestor_link_swap_after_authorization() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        let safe = workspace.join("safe");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&safe).expect("safe directory");
        fs::create_dir_all(&outside).expect("outside directory");
        fs::write(safe.join("report.txt"), "authorized").expect("safe file");
        fs::write(outside.join("report.txt"), "secret").expect("outside file");

        let alias = workspace.join("alias");
        create_directory_link(&safe, &alias);
        let config = AgentConfig {
            folder: workspace.to_string_lossy().into_owned(),
            ..AgentConfig::default()
        };
        let service = AuthorizedRootService::from_agent_config(&config).expect("valid root");
        let authorized = service
            .authorize_existing_file(&alias.join("report.txt"))
            .expect("initially authorized file");

        replace_directory_link(&outside, &alias);

        assert_eq!(
            FileContentDescriptorV1::from_authorized_path(
                &authorized,
                &FileResourceLimits::default(),
            )
            .expect_err("retargeted ancestor must fail closed")
            .code(),
            "unauthorized_path"
        );
    }

    #[test]
    fn descriptor_reports_file_that_changes_during_every_scan_attempt() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("changing.txt");
        fs::write(&path, vec![b'a'; SCAN_BUFFER_SIZE * 2]).expect("fixture file");
        let service = AuthorizedRootService::from_agent_config(&AgentConfig {
            folder: temp.path().to_string_lossy().into_owned(),
            ..AgentConfig::default()
        })
        .expect("valid root");
        let authorized = service
            .authorize_existing_file(&path)
            .expect("authorized fixture");
        let mut last_mutated_attempt = None;

        let error = FileContentDescriptorV1::from_authorized_path_with_hook(
            &authorized,
            &FileResourceLimits::default(),
            |attempt, bytes_read| {
                if bytes_read > 0 && last_mutated_attempt != Some(attempt) {
                    let mut writer = OpenOptions::new()
                        .append(true)
                        .open(&path)
                        .expect("mutation handle");
                    writer.write_all(b"x").expect("mutate during scan");
                    writer.sync_data().expect("persist mutation");
                    last_mutated_attempt = Some(attempt);
                }
            },
        )
        .expect_err("every attempted revision changed");

        assert_eq!(error.code(), "unstable_file");
        assert_eq!(last_mutated_attempt, Some(DESCRIPTOR_SCAN_ATTEMPTS - 1));
    }

    #[cfg(unix)]
    fn create_directory_link(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).expect("directory symlink");
    }

    #[cfg(windows)]
    fn create_directory_link(target: &Path, link: &Path) {
        junction::create(target, link).expect("directory junction");
    }

    #[cfg(unix)]
    fn replace_directory_link(target: &Path, link: &Path) {
        fs::remove_file(link).expect("remove directory symlink");
        create_directory_link(target, link);
    }

    #[cfg(windows)]
    fn replace_directory_link(target: &Path, link: &Path) {
        junction::delete(link).expect("remove directory junction");
        fs::remove_dir(link).expect("remove empty junction directory");
        create_directory_link(target, link);
    }

    #[test]
    fn detects_utf8_text_and_markdown_with_hash_and_metadata() {
        let text = describe("notes.txt", b"first\nsecond\n");
        assert_eq!(text.schema, 1);
        assert_eq!(text.display_name, "notes.txt");
        assert_eq!(text.extension.as_deref(), Some("txt"));
        assert_eq!(text.mime_type, "text/plain");
        assert_eq!(text.encoding.as_deref(), Some("utf-8"));
        assert_eq!(text.renderer_kind, FileRendererKind::Text);
        assert_eq!(text.size_bytes, 13);
        assert_eq!(text.line_count, Some(3));
        assert_eq!(
            text.content_hash,
            "sha256:dbea9325179efe46ea2add94f7b6b745ca983fabb208dc6d34aa064623d7ee23"
        );
        assert!(text.modified_at_ms > 0);
        assert!(text.capabilities.preview);
        assert!(text.unavailable_reason.is_none());

        let markdown = describe("README.MD", b"# Wardian\n");
        assert_eq!(markdown.extension.as_deref(), Some("md"));
        assert_eq!(markdown.mime_type, "text/markdown");
        assert_eq!(markdown.renderer_kind, FileRendererKind::Markdown);
    }

    #[test]
    fn text_line_count_matches_complete_editor_model_lines() {
        let cases: &[(&str, &[u8], u64)] = &[
            ("empty.txt", b"", 1),
            ("lf.txt", b"one\n", 2),
            ("crlf.txt", b"one\r\ntwo", 2),
            ("cr.txt", b"one\rtwo", 2),
        ];

        for (name, bytes, expected_lines) in cases {
            assert_eq!(
                describe(name, bytes).line_count,
                Some(*expected_lines),
                "{name}"
            );
        }
    }

    #[test]
    fn signatures_confirm_supported_images_and_pdf() {
        let cases: &[(&str, &[u8], &str, FileRendererKind)] = &[
            (
                "image.bin",
                b"\x89PNG\r\n\x1a\nrest",
                "image/png",
                FileRendererKind::Image,
            ),
            (
                "image.bin",
                b"\xff\xd8\xff\xe0rest",
                "image/jpeg",
                FileRendererKind::Image,
            ),
            (
                "image.bin",
                b"GIF89a\x01\x00\x01\x00rest",
                "image/gif",
                FileRendererKind::Image,
            ),
            (
                "image.bin",
                b"RIFF\x04\x00\x00\x00WEBPrest",
                "image/webp",
                FileRendererKind::Image,
            ),
            (
                "document.bin",
                b"%PDF-1.7\nrest",
                "application/pdf",
                FileRendererKind::Pdf,
            ),
        ];

        for (name, bytes, mime_type, renderer_kind) in cases {
            let descriptor = describe(name, bytes);
            assert_eq!(&descriptor.mime_type, mime_type);
            assert_eq!(&descriptor.renderer_kind, renderer_kind);
        }
    }

    #[test]
    fn images_without_parseable_dimensions_fail_closed() {
        let cases: &[(&str, &[u8])] = &[
            ("truncated.png", b"\x89PNG\r\n\x1a\n"),
            ("truncated.jpg", b"\xff\xd8\xff\xe0"),
            ("truncated.webp", b"RIFF\x04\x00\x00\x00WEBP"),
        ];

        for (name, bytes) in cases {
            let descriptor = describe(name, bytes);
            assert_eq!(descriptor.renderer_kind, FileRendererKind::Image);
            assert!(!descriptor.capabilities.preview, "{name}");
            assert!(!descriptor.capabilities.stream, "{name}");
            assert_eq!(
                descriptor.unavailable_reason.as_deref(),
                Some("image_dimensions_unavailable"),
                "{name}"
            );
        }
    }

    #[test]
    fn descriptor_scan_streams_content_through_bounded_buffers() {
        use std::io::{Cursor, Read};

        struct GuardedReader {
            inner: Cursor<Vec<u8>>,
            largest_request: usize,
        }

        impl Read for GuardedReader {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.largest_request = self.largest_request.max(buffer.len());
                assert!(buffer.len() <= 64 * 1024, "unbounded read request");
                self.inner.read(buffer)
            }
        }

        let bytes = b"line\n".repeat(100_000);
        let mut reader = GuardedReader {
            inner: Cursor::new(bytes.clone()),
            largest_request: 0,
        };
        let scan = scan_reader(&mut reader, 32).expect("streamed scan");

        assert_eq!(reader.largest_request, 64 * 1024);
        assert_eq!(scan.probe, bytes[..32]);
        assert_eq!(scan.line_count, Some(100_001));
        assert!(scan.is_utf8_text);
        assert_eq!(
            scan.content_hash,
            format!("sha256:{:x}", Sha256::digest(&bytes))
        );
    }

    #[test]
    fn descriptor_scan_stops_at_the_detected_renderer_byte_ceiling() {
        use std::io::{Cursor, Read};

        struct GuardedReader {
            inner: Cursor<Vec<u8>>,
            total_read: u64,
        }

        impl Read for GuardedReader {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let bytes_read = self.inner.read(buffer)?;
                self.total_read += bytes_read as u64;
                Ok(bytes_read)
            }
        }

        let limits = FileResourceLimits {
            monaco_max_size_bytes: 128,
            monaco_max_line_count: u64::MAX,
            diff_max_size_bytes_per_side: 128,
            diff_max_line_count: u64::MAX,
            image_max_size_bytes: 256,
            image_max_pixels: u64::MAX,
            pdf_max_size_bytes: 512,
        };
        let cases: &[(&str, &[u8], u64)] = &[
            ("text", b"plain text", limits.monaco_max_size_bytes),
            ("image", b"\x89PNG\r\n\x1a\n", limits.image_max_size_bytes),
            ("pdf", b"%PDF-1.7\n", limits.pdf_max_size_bytes),
        ];

        for (kind, signature, ceiling) in cases {
            let mut bytes = signature.to_vec();
            bytes.resize(4_096, b'a');
            let mut reader = GuardedReader {
                inner: Cursor::new(bytes),
                total_read: 0,
            };

            let error = scan_reader_limited(&mut reader, SIGNATURE_PROBE_SIZE, &limits, |_| {})
                .expect_err("content beyond its renderer ceiling must stop");

            assert_eq!(error.code(), "file_too_large", "{kind}");
            assert!(
                reader.total_read <= ceiling + 1,
                "{kind} read {} bytes past a {ceiling} byte ceiling",
                reader.total_read,
            );
        }

        let tiny_limits = FileResourceLimits {
            monaco_max_size_bytes: 8,
            ..limits
        };
        let mut reader = GuardedReader {
            inner: Cursor::new(vec![b'a'; 4_096]),
            total_read: 0,
        };
        let error = scan_reader_limited(&mut reader, SIGNATURE_PROBE_SIZE, &tiny_limits, |_| {})
            .expect_err("detection allowance must remain bounded");
        assert_eq!(error.code(), "file_too_large");
        assert_eq!(reader.total_read, MINIMUM_DETECTION_BYTES);
    }

    #[test]
    fn streaming_utf8_validation_handles_split_scalars_and_incomplete_input() {
        let mut valid = Utf8TextAnalyzer::default();
        valid.push(&[0xf0, 0x9f]);
        valid.push(&[0xa6, 0x80, b'\n']);
        assert_eq!(valid.finish(), (true, Some(2)));

        let mut incomplete = Utf8TextAnalyzer::default();
        incomplete.push(&[0xf0, 0x9f]);
        assert_eq!(incomplete.finish(), (false, None));

        let mut binary_control = Utf8TextAnalyzer::default();
        binary_control.push("before\u{0085}after".as_bytes());
        assert_eq!(binary_control.finish(), (false, None));
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_rejects_non_utf8_canonical_identity() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join(OsString::from_vec(vec![b'f', 0xff]));
        fs::write(&path, b"text").expect("fixture file");
        let service = AuthorizedRootService::from_agent_config(&AgentConfig {
            folder: temp.path().to_string_lossy().into_owned(),
            ..AgentConfig::default()
        })
        .expect("valid root");
        let authorized = service
            .authorize_existing_file(&path)
            .expect("authorized fixture");

        assert_eq!(
            FileContentDescriptorV1::from_authorized_path(
                &authorized,
                &FileResourceLimits::default(),
            )
            .expect_err("lossy resource identity must be rejected")
            .code(),
            "unavailable_path"
        );
    }

    #[test]
    fn encoded_image_dimensions_enforce_the_pixel_limit_boundary() {
        fn png_header(width: u32, height: u32) -> Vec<u8> {
            let mut bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
            bytes.extend_from_slice(&width.to_be_bytes());
            bytes.extend_from_slice(&height.to_be_bytes());
            bytes
        }

        fn gif_header(width: u16, height: u16) -> Vec<u8> {
            let mut bytes = b"GIF89a".to_vec();
            bytes.extend_from_slice(&width.to_le_bytes());
            bytes.extend_from_slice(&height.to_le_bytes());
            bytes
        }

        fn jpeg_header(width: u16, height: u16) -> Vec<u8> {
            let mut bytes = b"\xff\xd8\xff\xc0\x00\x08\x08".to_vec();
            bytes.extend_from_slice(&height.to_be_bytes());
            bytes.extend_from_slice(&width.to_be_bytes());
            bytes.push(0);
            bytes
        }

        fn webp_header(width: u32, height: u32) -> Vec<u8> {
            let mut bytes =
                b"RIFF\x16\x00\x00\x00WEBPVP8X\x0a\x00\x00\x00\x00\x00\x00\x00".to_vec();
            for value in [width - 1, height - 1] {
                bytes.extend_from_slice(&value.to_le_bytes()[..3]);
            }
            bytes
        }

        let fixtures = [
            ("png", png_header(8_000, 8_000), png_header(8_001, 8_000)),
            ("gif", gif_header(8_000, 8_000), gif_header(8_001, 8_000)),
            ("jpeg", jpeg_header(8_000, 8_000), jpeg_header(8_001, 8_000)),
            ("webp", webp_header(8_000, 8_000), webp_header(8_001, 8_000)),
        ];

        for (extension, at_limit_bytes, over_limit_bytes) in fixtures {
            let at_limit = describe(&format!("at-limit.{extension}"), &at_limit_bytes);
            assert!(at_limit.capabilities.preview, "{extension} at limit");

            let over_limit = describe(&format!("over-limit.{extension}"), &over_limit_bytes);
            assert!(!over_limit.capabilities.preview, "{extension} over limit");
            assert_eq!(
                over_limit.unavailable_reason.as_deref(),
                Some("image_limit_exceeded"),
                "{extension} over-limit reason"
            );
        }
    }

    #[test]
    fn extension_is_only_a_hint_and_binary_content_is_unsupported() {
        let false_png = describe("not-really.png", b"plain UTF-8 text\n");
        assert_eq!(false_png.mime_type, "text/plain");
        assert_eq!(false_png.renderer_kind, FileRendererKind::Text);

        let binary = describe("payload.txt", &[0, 1, 2, 0xff]);
        assert_eq!(binary.mime_type, "application/octet-stream");
        assert_eq!(binary.encoding, None);
        assert_eq!(binary.renderer_kind, FileRendererKind::Unsupported);
        assert_eq!(binary.line_count, None);
        assert!(!binary.capabilities.preview);
        assert_eq!(
            binary.unavailable_reason.as_deref(),
            Some("unsupported_content")
        );
    }

    #[test]
    fn limits_accept_each_boundary_and_reject_one_over() {
        let limits = FileResourceLimits::default();

        assert!(limits.allows_monaco(limits.monaco_max_size_bytes, limits.monaco_max_line_count));
        assert!(!limits.allows_monaco(limits.monaco_max_size_bytes + 1, 1));
        assert!(!limits.allows_monaco(1, limits.monaco_max_line_count + 1));

        assert!(limits.allows_diff_side(
            limits.diff_max_size_bytes_per_side,
            limits.diff_max_line_count,
        ));
        assert!(!limits.allows_diff_side(limits.diff_max_size_bytes_per_side + 1, 1));
        assert!(!limits.allows_diff_side(1, limits.diff_max_line_count + 1));

        assert!(limits.allows_image(limits.image_max_size_bytes, limits.image_max_pixels));
        assert!(!limits.allows_image(limits.image_max_size_bytes + 1, 1));
        assert!(!limits.allows_image(1, limits.image_max_pixels + 1));

        assert!(limits.allows_pdf(limits.pdf_max_size_bytes));
        assert!(!limits.allows_pdf(limits.pdf_max_size_bytes + 1));
    }

    #[test]
    fn defaults_and_dtos_serialize_with_stable_snake_case_values() {
        let limits = FileResourceLimits::default();
        assert_eq!(limits.monaco_max_size_bytes, 16 * 1024 * 1024);
        assert_eq!(limits.monaco_max_line_count, 200_000);
        assert_eq!(limits.diff_max_size_bytes_per_side, 5 * 1024 * 1024);
        assert_eq!(limits.diff_max_line_count, 100_000);
        assert_eq!(limits.image_max_size_bytes, 64 * 1024 * 1024);
        assert_eq!(limits.image_max_pixels, 64_000_000);
        assert_eq!(limits.pdf_max_size_bytes, 256 * 1024 * 1024);

        assert_eq!(
            serde_json::to_value(FileRendererKind::Markdown).expect("serialize renderer"),
            json!("markdown")
        );
        let error = FileResourceErrorV1::new("unavailable_path", "file is missing");
        assert_eq!(
            serde_json::to_value(error).expect("serialize error"),
            json!({
                "schema": 1,
                "code": "unavailable_path",
                "message": "file is missing"
            })
        );
    }
}
