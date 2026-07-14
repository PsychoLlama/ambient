//! Shared utilities for the LSP server.

use std::path::PathBuf;

use lsp_types::Uri;

/// Convert a file:// URI to a file path.
pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let uri_str = uri.as_str();
    if !uri_str.starts_with("file://") {
        return None;
    }

    let path_str = uri_str.strip_prefix("file://")?;
    Some(PathBuf::from(percent_decode(path_str)))
}

/// Convert a file path to a file:// URI.
///
/// The single mint point for every server-produced `file://` URI. Paths are
/// lexically normalized first (`ambient_analysis::package::lexically_normalize`)
/// so a `.`/`..` segment — e.g. from a `[build] src = "./"` manifest — never
/// reaches the wire: a minted URI must spell a file exactly as an editor-sent
/// URI would, or raw-string URI comparison silently fails.
pub fn path_to_uri(path: &std::path::Path) -> Option<Uri> {
    let normalized = ambient_analysis::package::lexically_normalize(path);
    let path_str = normalized.to_str()?;
    let encoded = percent_encode(path_str);
    let uri_str = format!("file://{encoded}");
    uri_str.parse().ok()
}

/// Decode percent-encoded characters in a URI path.
pub fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2
                && let Ok(byte) = u8::from_str_radix(&hex, 16)
            {
                result.push(byte as char);
                continue;
            }
            result.push('%');
            result.push_str(&hex);
        } else {
            result.push(c);
        }
    }

    result
}

/// Percent-encode special characters in a path for URI.
pub fn percent_encode(s: &str) -> String {
    use std::fmt::Write;

    let mut result = String::with_capacity(s.len());

    for c in s.chars() {
        match c {
            ' ' => result.push_str("%20"),
            '#' => result.push_str("%23"),
            '?' => result.push_str("%3F"),
            '/' | ':' | '-' | '_' | '.' | '~' => result.push(c),
            c if c.is_ascii_alphanumeric() => result.push(c),
            c => {
                for byte in c.to_string().as_bytes() {
                    let _ = write!(result, "%{byte:02X}");
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uri_to_path() {
        let uri: Uri = "file:///home/user/test.ab".parse().unwrap();
        let path = uri_to_path(&uri);
        assert_eq!(path, Some(PathBuf::from("/home/user/test.ab")));
    }

    #[test]
    fn test_uri_to_path_with_spaces() {
        let uri: Uri = "file:///home/user/my%20project/test.ab".parse().unwrap();
        let path = uri_to_path(&uri);
        assert_eq!(path, Some(PathBuf::from("/home/user/my project/test.ab")));
    }

    #[test]
    fn test_path_to_uri() {
        let path = std::path::Path::new("/home/user/test.ab");
        let uri = path_to_uri(path);
        assert!(uri.is_some());
        assert_eq!(uri.unwrap().as_str(), "file:///home/user/test.ab");
    }

    #[test]
    fn test_path_to_uri_with_spaces() {
        let path = std::path::Path::new("/home/user/my project/test.ab");
        let uri = path_to_uri(path);
        assert!(uri.is_some());
        assert!(uri.unwrap().as_str().contains("%20"));
    }

    #[test]
    fn test_percent_decode() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("test%2Fab"), "test/ab");
        assert_eq!(percent_decode("no%encoding"), "no%encoding"); // invalid sequence preserved
    }

    #[test]
    fn test_percent_encode() {
        assert_eq!(percent_encode("hello world"), "hello%20world");
        assert_eq!(percent_encode("/path/to/file.ab"), "/path/to/file.ab");
    }
}
