//! Immutable source transport for the embedded editor.
//!
//! Zed owns line layout and previews. Retaining a second per-line representation
//! here would duplicate its work and every loaded trace's text.
const PREVIEW_CHARACTERS: usize = 4096;

#[derive(Debug)]
pub struct CodeContent {
    source: String,
    pub clipped: bool,
}

impl CodeContent {
    pub fn new(text: String) -> Self {
        // The document loader needs this flag before enabling editing. Count
        // Unicode scalar values, preserving the existing CRLF/line semantics.
        // Stop at the first over-limit line; no line copies or index allocations.
        let clipped = text
            .lines()
            .any(|line| line.chars().nth(PREVIEW_CHARACTERS).is_some());
        Self {
            source: text,
            clipped,
        }
    }

    /// Exact bytes read from the source, including CRLF and final line endings.
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn is_empty(&self) -> bool {
        self.source.is_empty()
    }
}

impl From<String> for CodeContent {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl Default for CodeContent {
    fn default() -> Self {
        Self::new(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::CodeContent;

    #[test]
    fn long_unicode_lines_remain_exact_while_editing_is_marked_limited() {
        let first = "日😀e\u{301}".repeat(1500);
        let source = format!("{first}\nsecond line");
        let content = CodeContent::new(source.clone());
        assert!(content.clipped);
        assert_eq!(content.source(), source);
    }

    #[test]
    fn line_limit_counts_scalars_instead_of_utf8_bytes() {
        let line = "😀".repeat(4096);
        let content = CodeContent::from(line.clone());
        assert!(!content.clipped);
        assert_eq!(content.source(), line);
        assert!(CodeContent::new(format!("{line}日")).clipped);
    }

    #[test]
    fn every_line_is_checked_with_standard_crlf_semantics() {
        let line = "x".repeat(4096);
        assert!(!CodeContent::new(format!("{line}\r\n{line}\n")).clipped);
        assert!(CodeContent::new(format!("short\r\n{line}x\n")).clipped);
        assert!(CodeContent::new(format!("{line}\rx")).clipped);
        assert!(CodeContent::default().is_empty());
        assert!(!CodeContent::new("\n".into()).is_empty());
        assert!(!CodeContent::new("\r\n".into()).is_empty());
    }

    #[test]
    fn copied_patch_preserves_lf_and_final_newline_exactly() {
        let patch = "diff --git a/name.txt b/name.txt\n--- a/name.txt\n+++ b/name.txt\n@@ -1 +1 @@\n-old\n+new\n";
        let content = CodeContent::new(patch.into());
        assert_eq!(content.source().as_bytes(), patch.as_bytes());
        assert!(content.source().ends_with('\n'));
    }

    #[test]
    fn copy_preserves_crlf_mixed_endings_and_trailing_empty_lines() {
        for original in [
            "",
            "\n",
            "\r\n",
            "line\n",
            "line\r\n",
            "line\n\n",
            "line\r\n\r\n",
            "first\r\nsecond\nthird\r\n\n",
            "日本😀\r\n\r\n",
            "\u{feff}line\r",
            "a\0b",
        ] {
            let content = CodeContent::new(original.into());
            assert_eq!(content.source().as_bytes(), original.as_bytes());
            assert_eq!(content.is_empty(), original.is_empty());
        }
    }

    #[test]
    fn source_ownership_is_transferred_without_copying() {
        let source = "same original allocation\r\n日本😀\n".repeat(1000);
        let pointer = source.as_ptr();
        let capacity = source.capacity();
        let content = CodeContent::new(source);
        assert_eq!(content.source().as_ptr(), pointer);
        assert_eq!(content.source.capacity(), capacity);
    }
}
