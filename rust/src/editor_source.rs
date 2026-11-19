//! Immutable exact-source data for an upstream Zed buffer adapter.
//!
//! Zed's pinned `LineEnding::normalize` replaces CRLF and bare CR with LF:
//! <https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/text/src/text.rs#L3663-L3682>.
//! The buffer's normalized byte offsets therefore cannot directly index original
//! evidence. This bridge retains the original text and maps selections back to
//! its exact bytes, including CRLF, BOMs and final newlines.
//!
//! This is initial, immutable source data, not an editing or serialization model.
//! Its offsets are valid only while the engine text matches `normalized()`.
//! Callers remain responsible for their existing source-retention bounds.

use std::{fmt, ops::Range, sync::Arc};

/// Original evidence and the LF-normalized text used to initialize an editor.
///
/// Construction scans the source once. The offset map stores one `usize` per
/// CRLF, rather than one entry per byte or character. Mapping an offset performs
/// a binary search over those entries; it never rescans the document.
#[derive(Clone, Debug)]
pub struct ExactSource {
    original: Arc<str>,
    normalized: Arc<str>,
    // A collapsed CRLF contributes its extra original byte at and after this
    // normalized boundary. The boundary before its LF maps before its CR.
    crlf_ends: Arc<[usize]>,
}

/// An invalid byte offset or selection in the normalized source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoundaryError {
    OutOfBounds { offset: usize, len: usize },
    NotCharBoundary { offset: usize },
    ReversedRange { start: usize, end: usize },
}

impl fmt::Display for BoundaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds { offset, len } => write!(
                formatter,
                "normalized byte offset {offset} exceeds source length {len}"
            ),
            Self::NotCharBoundary { offset } => write!(
                formatter,
                "normalized byte offset {offset} is not a UTF-8 character boundary"
            ),
            Self::ReversedRange { start, end } => write!(
                formatter,
                "normalized selection starts at {start} after its end at {end}"
            ),
        }
    }
}

impl std::error::Error for BoundaryError {}

impl ExactSource {
    /// Retains `original` and normalizes precisely CRLF and bare CR to LF.
    ///
    /// Text without CR reuses the original allocation. BOMs, Unicode line and
    /// paragraph separators, and every other character remain unchanged.
    pub fn new(original: Arc<str>) -> Self {
        let bytes = original.as_bytes();
        let Some(first_cr) = bytes.iter().position(|byte| *byte == b'\r') else {
            return Self {
                normalized: Arc::clone(&original),
                original,
                crlf_ends: Arc::from([]),
            };
        };

        let mut normalized = String::with_capacity(original.len());
        let mut crlf_ends = Vec::new();
        let mut copy_start = 0;
        let mut offset = first_cr;
        while offset < bytes.len() {
            if bytes[offset] != b'\r' {
                offset += 1;
                continue;
            }

            // CR is ASCII, so the slice endpoints are UTF-8 boundaries even
            // though the scan itself visits every byte.
            normalized.push_str(&original[copy_start..offset]);
            normalized.push('\n');
            offset += 1;
            if bytes.get(offset) == Some(&b'\n') {
                offset += 1;
                crlf_ends.push(normalized.len());
            }
            copy_start = offset;
        }
        normalized.push_str(&original[copy_start..]);

        Self {
            original,
            normalized: Arc::from(normalized),
            crlf_ends: Arc::from(crlf_ends),
        }
    }

    /// The exact retained source, before normalization.
    pub fn original(&self) -> &str {
        &self.original
    }

    /// Text suitable for initializing the pinned Zed buffer.
    pub fn normalized(&self) -> &str {
        &self.normalized
    }

    /// Shares the original allocation without copying its text.
    pub fn original_arc(&self) -> Arc<str> {
        Arc::clone(&self.original)
    }

    /// Shares the normalized allocation without copying its text.
    pub fn normalized_arc(&self) -> Arc<str> {
        Arc::clone(&self.normalized)
    }

    /// Maps a normalized UTF-8 byte boundary to the exact original boundary.
    ///
    /// The boundary before an LF produced from CRLF maps before CR; the one
    /// after LF maps after the complete CRLF. An empty range stays empty.
    pub fn original_offset(&self, normalized_offset: usize) -> Result<usize, BoundaryError> {
        if normalized_offset > self.normalized.len() {
            return Err(BoundaryError::OutOfBounds {
                offset: normalized_offset,
                len: self.normalized.len(),
            });
        }
        if !self.normalized.is_char_boundary(normalized_offset) {
            return Err(BoundaryError::NotCharBoundary {
                offset: normalized_offset,
            });
        }

        let collapsed = self
            .crlf_ends
            .partition_point(|end| *end <= normalized_offset);
        Ok(normalized_offset + collapsed)
    }

    /// Maps an ordered normalized range after checking both byte boundaries.
    ///
    /// Reversed ranges are rejected first. Invalid offsets are never clipped.
    pub fn original_range(&self, normalized: Range<usize>) -> Result<Range<usize>, BoundaryError> {
        if normalized.start > normalized.end {
            return Err(BoundaryError::ReversedRange {
                start: normalized.start,
                end: normalized.end,
            });
        }
        Ok(self.original_offset(normalized.start)?..self.original_offset(normalized.end)?)
    }

    /// Borrows the exact original bytes covered by a normalized selection.
    ///
    /// A selected normalized LF recovers its original LF, CR, or CRLF. The
    /// adapter must use this result for evidence copy instead of upstream's
    /// normalized clipboard text, and must reject stale engine revisions.
    pub fn selected_text(&self, normalized: Range<usize>) -> Result<&str, BoundaryError> {
        Ok(&self.original[self.original_range(normalized)?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> ExactSource {
        ExactSource::new(Arc::from(text))
    }

    #[test]
    fn normalizes_crlf_and_bare_cr_without_losing_original_text() {
        let cases = [
            ("", ""),
            ("plain", "plain"),
            ("a\nb\n", "a\nb\n"),
            ("a\r\nb\r\n", "a\nb\n"),
            ("a\rb\r", "a\nb\n"),
            ("a\r\nb\rc\n\r\nz\r", "a\nb\nc\n\nz\n"),
            ("\r\r\n\n\r", "\n\n\n\n"),
            ("\r\n\r\n", "\n\n"),
        ];
        for (original, normalized) in cases {
            let shared: Arc<str> = Arc::from(original);
            let exact = ExactSource::new(Arc::clone(&shared));
            assert_eq!(exact.original(), original);
            assert_eq!(exact.normalized(), normalized);
            assert!(Arc::ptr_eq(&shared, &exact.original_arc()));
            assert_eq!(exact.selected_text(0..normalized.len()), Ok(original));
        }
    }

    #[test]
    fn preserves_bom_unicode_separators_nul_and_final_newline() {
        let original = "\u{feff}é\u{2028}中\u{2029}\0🦀\r\n";
        let exact = source(original);
        assert_eq!(exact.normalized(), "\u{feff}é\u{2028}中\u{2029}\0🦀\n");
        assert_eq!(
            exact.selected_text(0..exact.normalized().len()),
            Ok(original)
        );
        assert_eq!(exact.selected_text(0..3), Ok("\u{feff}"));
        let end = exact.normalized().len();
        assert_eq!(exact.selected_text(end - 1..end), Ok("\r\n"));
    }

    #[test]
    fn unchanged_source_aliases_the_original_allocation() {
        for original in ["", "line\n", "\u{feff}emoji🦀 e\u{301} 中\u{2028}\u{2029}"] {
            let shared: Arc<str> = Arc::from(original);
            let exact = ExactSource::new(Arc::clone(&shared));
            assert!(Arc::ptr_eq(&shared, &exact.normalized_arc()));
            assert!(exact.crlf_ends.is_empty());
        }
    }

    #[test]
    fn bare_cr_changes_the_text_even_without_a_length_change() {
        let exact = source("é\r中\r");
        assert_eq!(exact.normalized(), "é\n中\n");
        assert!(!Arc::ptr_eq(&exact.original_arc(), &exact.normalized_arc()));
        assert!(exact.crlf_ends.is_empty());
        assert_eq!(exact.original_offset(3), Ok(3));
        assert_eq!(exact.selected_text(2..3), Ok("\r"));
    }

    #[test]
    fn maps_crlf_boundaries_and_cross_line_selection_exactly() {
        let exact = source("a\r\nb\r\nc\n");
        assert_eq!(exact.normalized(), "a\nb\nc\n");
        assert_eq!(exact.crlf_ends.as_ref(), &[2, 4]);
        let expected = [0, 1, 3, 4, 6, 7, 8];
        for (normalized, original) in expected.into_iter().enumerate() {
            assert_eq!(exact.original_offset(normalized), Ok(original));
            assert_eq!(exact.selected_text(normalized..normalized), Ok(""));
        }
        assert_eq!(exact.selected_text(1..2), Ok("\r\n"));
        assert_eq!(exact.selected_text(2..3), Ok("b"));
        assert_eq!(exact.selected_text(1..5), Ok("\r\nb\r\nc"));
        assert_eq!(exact.selected_text(5..6), Ok("\n"));
    }

    #[test]
    fn every_valid_unicode_boundary_and_range_maps_to_expected_source() {
        // Explicit expected units keep this oracle independent of the map's
        // collapsed-newline counting algorithm.
        let units = [
            ("\u{feff}", "\u{feff}"),
            ("A", "A"),
            ("é", "é"),
            ("\r\n", "\n"),
            ("🦀", "🦀"),
            ("\r", "\n"),
            ("中", "中"),
            ("\n", "\n"),
            ("x", "x"),
            ("\u{301}", "\u{301}"),
            ("\r\n", "\n"),
            ("終", "終"),
            ("\r", "\n"),
        ];
        let original = units
            .iter()
            .map(|(original, _)| *original)
            .collect::<String>();
        let normalized = units
            .iter()
            .map(|(_, normalized)| *normalized)
            .collect::<String>();
        let exact = ExactSource::new(Arc::from(original.as_str()));
        assert_eq!(exact.normalized(), normalized);

        let mut boundaries = vec![(0, 0)];
        let mut original_offset = 0;
        let mut normalized_offset = 0;
        for (original_unit, normalized_unit) in units {
            let start = normalized_offset;
            original_offset += original_unit.len();
            normalized_offset += normalized_unit.len();
            boundaries.push((normalized_offset, original_offset));
            assert_eq!(
                exact.selected_text(start..normalized_offset),
                Ok(original_unit)
            );
        }
        assert_eq!(
            exact
                .normalized()
                .char_indices()
                .map(|(offset, _)| offset)
                .chain([normalized.len()])
                .collect::<Vec<_>>(),
            boundaries
                .iter()
                .map(|(normalized, _)| *normalized)
                .collect::<Vec<_>>()
        );
        for (start_index, &(start, original_start)) in boundaries.iter().enumerate() {
            assert_eq!(exact.original_offset(start), Ok(original_start));
            for &(end, original_end) in &boundaries[start_index..] {
                assert_eq!(
                    exact.original_range(start..end),
                    Ok(original_start..original_end)
                );
                assert_eq!(
                    exact.selected_text(start..end),
                    Ok(&original[original_start..original_end])
                );
            }
        }
    }

    #[test]
    fn rejects_every_interior_utf8_byte_offset() {
        let exact = source("\u{feff}é\r\n🦀\r中e\u{301}");
        for offset in 0..=exact.normalized().len() {
            if exact.normalized().is_char_boundary(offset) {
                assert!(exact.original_offset(offset).is_ok());
                continue;
            }
            let error = BoundaryError::NotCharBoundary { offset };
            assert_eq!(exact.original_offset(offset), Err(error.clone()));
            assert_eq!(exact.original_range(0..offset), Err(error.clone()));
            assert_eq!(
                exact.original_range(offset..exact.normalized().len()),
                Err(error.clone())
            );
            assert_eq!(exact.selected_text(offset..offset), Err(error));
        }
    }

    #[test]
    fn rejects_out_of_bounds_offsets_and_ranges_without_clipping() {
        let exact = source("é\r\n");
        let len = exact.normalized().len();
        for offset in [len + 1, usize::MAX] {
            let error = BoundaryError::OutOfBounds { offset, len };
            assert_eq!(exact.original_offset(offset), Err(error.clone()));
            assert_eq!(exact.original_range(0..offset), Err(error.clone()));
            assert_eq!(exact.original_range(offset..offset), Err(error.clone()));
            assert_eq!(exact.selected_text(0..offset), Err(error));
        }
        let empty = source("");
        assert_eq!(empty.original_offset(0), Ok(0));
        assert_eq!(empty.selected_text(0..0), Ok(""));
        assert_eq!(
            empty.original_offset(1),
            Err(BoundaryError::OutOfBounds { offset: 1, len: 0 })
        );
    }

    #[test]
    fn rejects_reversed_ranges_before_mapping_boundaries() {
        let exact = source("é\r\n");
        for (start, end) in [(3, 0), (1, 0), (usize::MAX, 1)] {
            let error = BoundaryError::ReversedRange { start, end };
            assert_eq!(exact.original_range(start..end), Err(error.clone()));
            assert_eq!(exact.selected_text(start..end), Err(error));
        }
    }

    #[test]
    fn selection_is_a_borrow_of_the_original_allocation() {
        let exact = source("start\r\né🦀\r\nend");
        let range = 6..12;
        let original_range = exact.original_range(range.clone()).unwrap();
        let selected = exact.selected_text(range).unwrap();
        assert_eq!(selected, "é🦀");
        assert_eq!(selected.as_ptr(), exact.original()[original_range].as_ptr());
    }

    #[test]
    fn clones_share_all_immutable_allocations() {
        let exact = source("a\r\nb\r\n");
        let cloned = exact.clone();
        assert!(Arc::ptr_eq(&exact.original, &cloned.original));
        assert!(Arc::ptr_eq(&exact.normalized, &cloned.normalized));
        assert!(Arc::ptr_eq(&exact.crlf_ends, &cloned.crlf_ends));
    }

    #[test]
    fn large_unchanged_source_uses_one_text_allocation() {
        let shared: Arc<str> = Arc::from("中🦀 e\u{301}\n".repeat(16_384));
        let exact = ExactSource::new(Arc::clone(&shared));
        assert!(Arc::ptr_eq(&shared, &exact.original_arc()));
        assert!(Arc::ptr_eq(&shared, &exact.normalized_arc()));
        assert!(exact.crlf_ends.is_empty());
        assert_eq!(
            exact.original_offset(exact.normalized().len()),
            Ok(shared.len())
        );
    }

    #[test]
    fn large_mixed_source_keeps_only_crlf_map_entries_and_reuses_them() {
        let repetitions = 8_192;
        let exact = source(&"中\r\n🦀\re\u{301}\n".repeat(repetitions));
        assert_eq!(exact.crlf_ends.len(), repetitions);
        assert_eq!(exact.normalized(), "中\n🦀\ne\u{301}\n".repeat(repetitions));
        let map_pointer = exact.crlf_ends.as_ptr();
        let normalized_pointer = exact.normalized().as_ptr();
        let unit_len = "中\n🦀\ne\u{301}\n".len();
        for repetition in 0..repetitions {
            let start = repetition * unit_len;
            assert_eq!(
                exact.selected_text(start..start + unit_len),
                Ok("中\r\n🦀\re\u{301}\n")
            );
        }
        assert_eq!(map_pointer, exact.crlf_ends.as_ptr());
        assert_eq!(normalized_pointer, exact.normalized().as_ptr());
        assert_eq!(
            exact.original_offset(exact.normalized().len()),
            Ok(exact.original().len())
        );
    }
}
