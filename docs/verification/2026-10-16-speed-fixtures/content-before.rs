//! Immutable source lines with bounded, precomputed previews for native rendering.
use std::ops::{Deref, Range};

const PREVIEW_CHARACTERS: usize = 4096;

#[derive(Debug)]
pub struct CodeContent {
    source: String,
    originals: Vec<String>,
    line_starts: Vec<usize>,
    preview_ends: Vec<usize>,
    pub width: f32,
    pub clipped: bool,
}

impl CodeContent {
    pub fn new(text: String) -> Self {
        let mut originals = Vec::new();
        let mut line_starts = Vec::new();
        let mut preview_ends = Vec::new();
        let mut widest_preview = 0_f32;
        let mut clipped = false;

        // Match the existing line-splitting semantics, including CRLF handling.
        // Stop counting each line at the preview limit; render needs no rescans.
        let mut source_offset = 0;
        for line in text.lines() {
            line_starts.push(source_offset);
            let mut count = 0;
            let mut preview_width = 0_f32;
            let mut end = line.len();
            for (byte, character) in line.char_indices() {
                if count == PREVIEW_CHARACTERS {
                    end = byte;
                    clipped = true;
                    break;
                }
                count += 1;
                // Menlo's ASCII advance is below 7.5 at 12px; reserve more for
                // fallback CJK/emoji glyphs so horizontal scrolling reaches them.
                preview_width += if character.is_ascii() { 7.5 } else { 16. };
            }
            widest_preview = widest_preview.max(preview_width);
            originals.push(line.to_owned());
            preview_ends.push(end);
            source_offset += line.len();
            source_offset += if text[source_offset..].starts_with("\r\n") {
                2
            } else if text[source_offset..].starts_with('\n') {
                1
            } else {
                0
            };
        }

        Self {
            source: text,
            originals,
            line_starts,
            preview_ends,
            width: widest_preview + 72.,
            clipped,
        }
    }

    /// Borrow at most 4096 Unicode scalar values, without copying or reshaping.
    /// Indexing follows Vec's bounds behavior; callers supply a visible row index.
    pub fn preview(&self, index: usize) -> &str {
        &self.originals[index][..self.preview_ends[index]]
    }

    /// Exact bytes read from the source, including CRLF and final line endings.
    /// Use this for Copy/export; joining display lines cannot preserve them.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Exact source byte range of a display line, excluding its line ending.
    pub fn line_range(&self, index: usize) -> Range<usize> {
        let start = self.line_starts[index];
        start..start + self.originals[index].len()
    }

    /// Exact source byte range including the original LF or CRLF, if present.
    pub fn full_line_range(&self, index: usize) -> Range<usize> {
        self.line_starts[index]
            ..self
                .line_starts
                .get(index + 1)
                .copied()
                .unwrap_or(self.source.len())
    }

    pub fn line_for_offset(&self, offset: usize) -> Option<usize> {
        if self.line_starts.is_empty() {
            return None;
        }
        Some(
            match self
                .line_starts
                .binary_search(&offset.min(self.source.len()))
            {
                Ok(index) => index,
                Err(index) => index.saturating_sub(1),
            },
        )
    }
}

// Keep originals immutable so line content and cached preview endpoints cannot
// drift apart. Deref exposes unabridged display lines; source() preserves bytes.
impl Deref for CodeContent {
    type Target = Vec<String>;

    fn deref(&self) -> &Self::Target {
        &self.originals
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
    fn long_unicode_preview_ends_at_a_scalar_boundary_without_truncating_copy() {
        let first = "日😀e\u{301}".repeat(1500);
        let source = format!("{first}\nsecond line");
        let content = CodeContent::new(source.clone());

        assert!(content.clipped);
        assert_eq!(content.preview(0).chars().count(), 4096);
        assert!(first.is_char_boundary(content.preview(0).len()));
        assert!(first.starts_with(content.preview(0)));
        assert_eq!(content[0], first);
        assert_eq!(content.preview(1), "second line");
        assert_eq!(content.source(), source);
        assert_eq!(content.join("\n"), source);
        assert!(content.width >= 4096. * 7.5 + 72.);
    }

    #[test]
    fn exact_limit_is_not_reported_as_clipped() {
        let line = "😀".repeat(4096);
        let content = CodeContent::from(line.clone());

        assert!(!content.clipped);
        assert_eq!(content.preview(0), line);
        let beyond = CodeContent::new(format!("{line}日"));
        assert!(beyond.clipped);
        assert_eq!(beyond.preview(0), line);
        assert_eq!(beyond[0].chars().count(), 4097);
    }

    #[test]
    fn width_uses_scalar_count_and_existing_line_semantics() {
        let content = CodeContent::new("日😀\r\nabc\n".into());

        assert_eq!(content.len(), 2);
        assert_eq!(content.preview(0), "日😀");
        assert_eq!(content.width, 2. * 16. + 72.);
        assert_eq!(content.join("\n"), "日😀\nabc");
        assert!(!content.clipped);
        let empty = CodeContent::default();
        assert!(empty.is_empty());
        assert_eq!(empty.width, 72.);
        assert!(!empty.clipped);
    }

    #[test]
    fn copied_patch_preserves_lf_and_final_newline_exactly() {
        let patch = "diff --git a/name.txt b/name.txt\n--- a/name.txt\n+++ b/name.txt\n@@ -1 +1 @@\n-old\n+new\n";
        let content = CodeContent::new(patch.into());

        assert_eq!(content.source().as_bytes(), patch.as_bytes());
        assert!(content.source().ends_with('\n'));
        assert_ne!(content.join("\n").as_bytes(), patch.as_bytes());
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
        ] {
            let content = CodeContent::new(original.into());
            assert_eq!(content.source().as_bytes(), original.as_bytes());
        }
        let content = CodeContent::new("日本😀\r\n\r\n".into());
        assert_eq!(content.len(), 2);
        assert_eq!(content.preview(1), "");
        assert_eq!(content.source(), "日本😀\r\n\r\n");
    }
}
