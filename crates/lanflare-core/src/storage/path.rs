//! Path sanitation and directory traversal defense.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Windows reserved device names that cannot be used as filenames or base stems.
const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL",
    "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Maximum allowed length in bytes for a single filename component.
pub const MAX_FILENAME_BYTES: usize = 255;

/// Errors arising during filename sanitization, validation, or path resolution.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum PathSanitizeError {
    #[error("Filename is empty or contains only whitespace")]
    EmptyFilename,

    #[error("Filename contains null byte or control character")]
    ContainsControlChar,

    #[error("Path traversal attempted: {0}")]
    PathTraversal(String),

    #[error("Filename contains Windows alternate data stream separator ':'")]
    AlternateDataStream,

    #[error("Filename matches Windows reserved device name: {0}")]
    ReservedDeviceName(String),

    #[error("Filename contains forbidden character: '{0}'")]
    ForbiddenCharacter(char),

    #[error("Filename exceeds maximum allowed length ({0} bytes > {MAX_FILENAME_BYTES})")]
    FilenameTooLong(usize),

    #[error("Resolved path escapes the download directory: {0}")]
    EscapesBaseDir(String),

    #[error("Failed to generate unique destination path after maximum attempts")]
    CollisionExhausted,
}

/// Checks if the given filename or stem matches a reserved Windows device name (case-insensitive).
///
/// Windows treats device names as reserved regardless of extension (e.g., `CON`, `con.txt`, `prn.log`).
pub fn is_reserved_windows_name(name: &str) -> bool {
    let clean = name.trim();
    if clean.is_empty() {
        return false;
    }
    // Extract stem before the first dot
    let stem = clean.split('.').next().unwrap_or(clean).trim();
    WINDOWS_RESERVED_NAMES
        .iter()
        .any(|&reserved| reserved.eq_ignore_ascii_case(stem))
}

/// Validates that a string is a safe, single-component filename without any sanitization or modification.
///
/// Rejects path separators (`/`, `\`), traversal (`..`, `.`), drive letters, alternate data streams (`:`),
/// control characters, null bytes, Windows forbidden characters (`<`, `>`, `"`, `|`, `?`, `*`),
/// trailing spaces/dots, reserved device names, and names exceeding 255 bytes.
pub fn validate_filename(name: &str) -> Result<(), PathSanitizeError> {
    if name.is_empty() || name.trim().is_empty() {
        return Err(PathSanitizeError::EmptyFilename);
    }

    if name.len() > MAX_FILENAME_BYTES {
        return Err(PathSanitizeError::FilenameTooLong(name.len()));
    }

    if name.chars().any(|c| c == '\0' || c.is_ascii_control()) {
        return Err(PathSanitizeError::ContainsControlChar);
    }

    if name.contains(':') {
        return Err(PathSanitizeError::AlternateDataStream);
    }

    if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(PathSanitizeError::PathTraversal(name.to_string()));
    }

    for c in name.chars() {
        if matches!(c, '<' | '>' | '"' | '|' | '?' | '*') {
            return Err(PathSanitizeError::ForbiddenCharacter(c));
        }
    }

    if name.ends_with(' ') || name.ends_with('.') {
        return Err(PathSanitizeError::ForbiddenCharacter(name.chars().last().unwrap()));
    }

    if is_reserved_windows_name(name) {
        return Err(PathSanitizeError::ReservedDeviceName(name.to_string()));
    }

    Ok(())
}

/// Sanitizes an untrusted filename string to produce a safe single-component filename.
///
/// Operations performed:
/// - Rejects null bytes and ASCII control characters.
/// - Splits on Unix and Windows path separators (`/`, `\`) and extracts the last non-empty basename.
/// - Rejects pure path traversal components (`.`, `..`).
/// - Rejects Windows Alternate Data Streams (`:`).
/// - Neutralizes Windows forbidden characters (`<`, `>`, `"`, `|`, `?`, `*`) by replacing them with `_`.
/// - Trims trailing whitespace and dots.
/// - Rejects Windows reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`).
/// - Enforces the 255-byte component limit.
pub fn sanitize_filename(name: &str) -> Result<String, PathSanitizeError> {
    if name.is_empty() {
        return Err(PathSanitizeError::EmptyFilename);
    }

    if name.contains('\0') {
        return Err(PathSanitizeError::ContainsControlChar);
    }

    // Split on path separators to extract the final basename
    let parts: Vec<&str> = name
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();

    if parts.is_empty() {
        return Err(PathSanitizeError::EmptyFilename);
    }

    let last = parts.last().copied().unwrap_or("");
    if last == "." || last == ".." {
        return Err(PathSanitizeError::PathTraversal(name.to_string()));
    }

    if last.contains(':') {
        return Err(PathSanitizeError::AlternateDataStream);
    }

    // Replace invalid characters and strip control characters
    let filtered: String = last
        .chars()
        .filter(|c| !c.is_ascii_control())
        .map(|c| match c {
            '<' | '>' | '"' | '|' | '?' | '*' => '_',
            other => other,
        })
        .collect();

    // Trim trailing spaces and dots
    let trimmed = filtered.trim_end_matches([' ', '.']).trim();
    if trimmed.is_empty() {
        return Err(PathSanitizeError::EmptyFilename);
    }

    if is_reserved_windows_name(trimmed) {
        return Err(PathSanitizeError::ReservedDeviceName(trimmed.to_string()));
    }

    if trimmed.len() > MAX_FILENAME_BYTES {
        return Err(PathSanitizeError::FilenameTooLong(trimmed.len()));
    }

    Ok(trimmed.to_string())
}

/// Resolves and secures a destination path inside `base_dir` from an untrusted filename.
///
/// Ensures the filename is sanitized and that the resolved path does not escape `base_dir`.
pub fn resolve_destination_path(base_dir: &Path, file_name: &str) -> Result<PathBuf, PathSanitizeError> {
    let sanitized = sanitize_filename(file_name)?;
    let resolved = base_dir.join(&sanitized);

    // Verify parent is base_dir to strictly forbid jailbreaks
    if let Some(parent) = resolved.parent()
        && parent != base_dir
        && !base_dir.as_os_str().is_empty()
    {
        return Err(PathSanitizeError::EscapesBaseDir(resolved.display().to_string()));
    }

    Ok(resolved)
}

/// Resolves a non-colliding destination path inside `base_dir`.
///
/// If `file_name` already exists in `base_dir`, appends an incremental counter
/// in the format `stem (1).ext`, `stem (2).ext`, etc.
pub fn resolve_unique_destination_path(
    base_dir: &Path,
    file_name: &str,
) -> Result<PathBuf, PathSanitizeError> {
    let sanitized = sanitize_filename(file_name)?;
    let target = base_dir.join(&sanitized);

    if !target.exists() {
        return Ok(target);
    }

    let stem = target
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let ext = target.extension().and_then(|e| e.to_str());

    for counter in 1..=10_000 {
        let candidate_name = match ext {
            Some(extension) => format!("{stem} ({counter}).{extension}"),
            None => format!("{stem} ({counter})"),
        };
        let candidate = base_dir.join(candidate_name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(PathSanitizeError::CollisionExhausted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_filename_valid() {
        assert!(validate_filename("photo.jpg").is_ok());
        assert!(validate_filename("my document (1).pdf").is_ok());
        assert!(validate_filename("archive_2026-10-13.tar.gz").is_ok());
        assert!(validate_filename("中文测试.png").is_ok());
        assert!(validate_filename("café_au_lait.txt").is_ok());
    }

    #[test]
    fn test_validate_filename_rejections() {
        // Path traversal
        assert!(matches!(
            validate_filename("../../etc/passwd"),
            Err(PathSanitizeError::PathTraversal(_))
        ));
        assert!(matches!(
            validate_filename(r"..\..\windows\system32"),
            Err(PathSanitizeError::PathTraversal(_))
        ));
        assert!(matches!(
            validate_filename(".."),
            Err(PathSanitizeError::PathTraversal(_))
        ));
        assert!(matches!(
            validate_filename("."),
            Err(PathSanitizeError::PathTraversal(_))
        ));

        // Windows drive letters & separators
        assert!(matches!(
            validate_filename(r"C:\Windows\System32"),
            Err(PathSanitizeError::AlternateDataStream)
        ));

        // Windows alternate data streams
        assert!(matches!(
            validate_filename("file.txt:stream"),
            Err(PathSanitizeError::AlternateDataStream)
        ));
        assert!(matches!(
            validate_filename("data.txt::$DATA"),
            Err(PathSanitizeError::AlternateDataStream)
        ));

        // Control characters & null bytes
        assert!(matches!(
            validate_filename("test\0file.txt"),
            Err(PathSanitizeError::ContainsControlChar)
        ));
        assert!(matches!(
            validate_filename("line\nbreak.txt"),
            Err(PathSanitizeError::ContainsControlChar)
        ));

        // Forbidden characters
        assert!(matches!(
            validate_filename("invalid<char>.txt"),
            Err(PathSanitizeError::ForbiddenCharacter('<'))
        ));
        assert!(matches!(
            validate_filename("what?.txt"),
            Err(PathSanitizeError::ForbiddenCharacter('?'))
        ));
        assert!(matches!(
            validate_filename("wild*card.txt"),
            Err(PathSanitizeError::ForbiddenCharacter('*'))
        ));

        // Trailing periods and spaces
        assert!(matches!(
            validate_filename("trailing. "),
            Err(PathSanitizeError::ForbiddenCharacter(' '))
        ));
        assert!(matches!(
            validate_filename("trailing."),
            Err(PathSanitizeError::ForbiddenCharacter('.'))
        ));

        // Empty
        assert!(matches!(
            validate_filename(""),
            Err(PathSanitizeError::EmptyFilename)
        ));
        assert!(matches!(
            validate_filename("   "),
            Err(PathSanitizeError::EmptyFilename)
        ));

        // Reserved Windows names
        assert!(matches!(
            validate_filename("CON"),
            Err(PathSanitizeError::ReservedDeviceName(_))
        ));
        assert!(matches!(
            validate_filename("con.txt"),
            Err(PathSanitizeError::ReservedDeviceName(_))
        ));
        assert!(matches!(
            validate_filename("NUL.dat"),
            Err(PathSanitizeError::ReservedDeviceName(_))
        ));
        assert!(matches!(
            validate_filename("AUX.tar.gz"),
            Err(PathSanitizeError::ReservedDeviceName(_))
        ));
        assert!(matches!(
            validate_filename("COM1.exe"),
            Err(PathSanitizeError::ReservedDeviceName(_))
        ));
        assert!(matches!(
            validate_filename("LPT9"),
            Err(PathSanitizeError::ReservedDeviceName(_))
        ));
    }

    #[test]
    fn test_sanitize_filename_neutralization() {
        // Directory traversal path neutralization
        assert_eq!(
            sanitize_filename("../../etc/passwd").unwrap(),
            "passwd"
        );
        assert_eq!(
            sanitize_filename(r"..\..\windows\system32\calc.exe").unwrap(),
            "calc.exe"
        );
        assert_eq!(
            sanitize_filename("/var/log/lanflare/output.log").unwrap(),
            "output.log"
        );

        // Windows forbidden character substitution
        assert_eq!(
            sanitize_filename("file<name>?with*bad|chars.txt").unwrap(),
            "file_name__with_bad_chars.txt"
        );

        // Trailing space and dot trimming
        assert_eq!(
            sanitize_filename("my_file.txt.   ").unwrap(),
            "my_file.txt"
        );
        assert_eq!(
            sanitize_filename("document....").unwrap(),
            "document"
        );

        // Pure traversal rejection
        assert!(matches!(
            sanitize_filename("../../"),
            Err(PathSanitizeError::PathTraversal(_))
        ));
        assert!(matches!(
            sanitize_filename(".."),
            Err(PathSanitizeError::PathTraversal(_))
        ));
        assert!(matches!(
            sanitize_filename("/"),
            Err(PathSanitizeError::EmptyFilename)
        ));

        // Alternate data stream rejection
        assert!(matches!(
            sanitize_filename("foo.txt:hidden"),
            Err(PathSanitizeError::AlternateDataStream)
        ));

        // Reserved device name rejection
        assert!(matches!(
            sanitize_filename("CON.txt"),
            Err(PathSanitizeError::ReservedDeviceName(_))
        ));
        assert!(matches!(
            sanitize_filename("path/to/prn.log"),
            Err(PathSanitizeError::ReservedDeviceName(_))
        ));

        // Null byte rejection
        assert!(matches!(
            sanitize_filename("exploit\0.png"),
            Err(PathSanitizeError::ContainsControlChar)
        ));
    }

    #[test]
    fn test_is_reserved_windows_name() {
        assert!(is_reserved_windows_name("CON"));
        assert!(is_reserved_windows_name("con"));
        assert!(is_reserved_windows_name("con.txt"));
        assert!(is_reserved_windows_name("PRN"));
        assert!(is_reserved_windows_name("prn.png"));
        assert!(is_reserved_windows_name("AUX"));
        assert!(is_reserved_windows_name("aux.tar.gz"));
        assert!(is_reserved_windows_name("NUL"));
        assert!(is_reserved_windows_name("COM1"));
        assert!(is_reserved_windows_name("com9.dll"));
        assert!(is_reserved_windows_name("LPT1"));
        assert!(is_reserved_windows_name("lpt5.doc"));

        // Non-reserved names
        assert!(!is_reserved_windows_name("contact.txt"));
        assert!(!is_reserved_windows_name("normal_file.pdf"));
        assert!(!is_reserved_windows_name("auxiliary.txt"));
        assert!(!is_reserved_windows_name("printer.jpg"));
        assert!(!is_reserved_windows_name(""));
    }

    #[test]
    fn test_resolve_destination_path() {
        let base = Path::new("/tmp/lanflare_downloads");
        let resolved = resolve_destination_path(base, "photo.jpg").unwrap();
        assert_eq!(resolved, base.join("photo.jpg"));

        // Traversal neutralized to basename
        let resolved_traversal = resolve_destination_path(base, "../../etc/passwd").unwrap();
        assert_eq!(resolved_traversal, base.join("passwd"));
    }

    #[test]
    fn test_resolve_unique_destination_path() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_unique_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let file1 = temp_dir.join("report.pdf");
        std::fs::write(&file1, b"first").unwrap();

        let unique1 = resolve_unique_destination_path(&temp_dir, "report.pdf").unwrap();
        assert_eq!(unique1, temp_dir.join("report (1).pdf"));

        std::fs::write(&unique1, b"second").unwrap();

        let unique2 = resolve_unique_destination_path(&temp_dir, "report.pdf").unwrap();
        assert_eq!(unique2, temp_dir.join("report (2).pdf"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
