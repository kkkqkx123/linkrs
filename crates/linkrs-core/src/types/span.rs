//! Source Code Location Type Definition
//!
//! This module defines generic source location types to represent the location of tokens and AST nodes in the source code.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Source Code Location
///
/// Indicates a point location in the source code and contains row and column numbers.
/// Row and column numbers are counted from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct Position {
    /// Line numbers, starting at 1
    pub line: usize,
    /// Column number, starting with 1
    pub column: usize,
}

impl Position {
    /// Creating a new location
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }

    /// Converting positions to character offsets
    ///
    /// # Parameters
    ///
    /// * `line_lengths` - array of character lengths per line
    ///
    /// # Return value
    ///
    /// If the line number is valid, the corresponding character offset is returned; otherwise, None is returned.
    pub fn to_offset(&self, line_lengths: &[usize]) -> Option<usize> {
        if self.line == 0 || self.line > line_lengths.len() {
            return None;
        }

        let mut offset = 0;
        for length in line_lengths.iter().take(self.line - 1) {
            offset += length + 1;
        }
        offset += self.column.saturating_sub(1);

        Some(offset)
    }

    /// Convert position to usize (for simple comparisons)
    pub fn to_usize(&self) -> usize {
        self.line * 1000 + self.column
    }

    /// Check that the position is valid (row and column numbers are both greater than 0)
    pub fn is_valid(&self) -> bool {
        self.line > 0 && self.column > 0
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.line, self.column)
    }
}

/// source code span
///
/// Indicates a range in the source code, from the start position to the end position.
/// Used to indicate the range of locations of tokens, expressions, statements, etc. in the source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Span {
    /// Starting position (included)
    pub start: Position,
    /// End position (included)
    pub end: Position,
}

impl Span {
    /// Creating a new span
    pub fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }

    /// Creating spans from four coordinates
    pub fn from_coords(
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
    ) -> Self {
        Self {
            start: Position::new(start_line, start_col),
            end: Position::new(end_line, end_col),
        }
    }

    /// Creating spans from a single location (for a single token)
    pub fn from_position(pos: Position) -> Self {
        Self {
            start: pos,
            end: pos,
        }
    }

    /// End position of the extended span
    pub fn extend(&mut self, other: Span) {
        self.end = other.end;
    }

    /// Combining two spans
    ///
    /// # Parameters
    ///
    /// * :: `other` -- another span to be combined
    ///
    /// # Return value
    ///
    /// A new span with the start position at the beginning of the current span and the end position at the end of the larger of the two spans
    pub fn merge(&self, other: Span) -> Span {
        Span::new(
            self.start,
            if self.end >= other.end {
                self.end
            } else {
                other.end
            },
        )
    }

    /// Check if span is empty (start position equals end position)
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Check if the span contains the specified position
    pub fn contains(&self, pos: Position) -> bool {
        self.start <= pos && pos <= self.end
    }

    /// Get the line number range of the span
    pub fn line_range(&self) -> (usize, usize) {
        (self.start.line, self.end.line)
    }

    /// Get the range of column numbers for the span (valid only if on the same row)
    pub fn column_range(&self) -> (usize, usize) {
        (self.start.column, self.end.column)
    }

    /// Get the line number of the starting position
    pub fn start_line(&self) -> usize {
        self.start.line
    }

    /// Get the column number of the starting position
    pub fn start_column(&self) -> usize {
        self.start.column
    }

    /// Get the line number of the end position
    pub fn end_line(&self) -> usize {
        self.end.line
    }

    /// Get the column number of the ending position
    pub fn end_column(&self) -> usize {
        self.end.column
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{} - {}:{}",
            self.start.line, self.start.column, self.end.line, self.end.column
        )
    }
}

/// Trait converted to Span
///
/// For easy conversion of location-dependent types to Span
pub trait ToSpan {
    fn to_span(&self) -> Span;
}

impl ToSpan for Position {
    fn to_span(&self) -> Span {
        Span::from_position(*self)
    }
}

impl ToSpan for Span {
    fn to_span(&self) -> Span {
        *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_creation() {
        let pos = Position::new(10, 5);
        assert_eq!(pos.line, 10);
        assert_eq!(pos.column, 5);
    }

    #[test]
    fn test_position_display() {
        let pos = Position::new(1, 1);
        assert_eq!(pos.to_string(), "(1, 1)");
    }

    #[test]
    fn test_position_to_usize() {
        let pos = Position::new(2, 3);
        assert_eq!(pos.to_usize(), 2003);
    }

    #[test]
    fn test_span_creation() {
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));
        assert_eq!(span.start.line, 1);
        assert_eq!(span.end.column, 10);
    }

    #[test]
    fn test_span_from_coords() {
        let span = Span::from_coords(1, 5, 2, 10);
        assert_eq!(span.start.line, 1);
        assert_eq!(span.start.column, 5);
        assert_eq!(span.end.line, 2);
        assert_eq!(span.end.column, 10);
    }

    #[test]
    fn test_span_from_position() {
        let pos = Position::new(5, 10);
        let span = Span::from_position(pos);
        assert!(span.is_empty());
    }

    #[test]
    fn test_span_extend() {
        let mut span = Span::new(Position::new(1, 1), Position::new(1, 5));
        span.extend(Span::new(Position::new(1, 6), Position::new(2, 10)));
        assert_eq!(span.end.line, 2);
        assert_eq!(span.end.column, 10);
    }

    #[test]
    fn test_span_merge() {
        let span1 = Span::new(Position::new(1, 1), Position::new(1, 5));
        let span2 = Span::new(Position::new(1, 6), Position::new(2, 10));
        let merged = span1.merge(span2);
        assert_eq!(merged.start, Position::new(1, 1));
        assert_eq!(merged.end, Position::new(2, 10));
    }

    #[test]
    fn test_span_is_empty() {
        let span = Span::new(Position::new(1, 1), Position::new(1, 1));
        assert!(span.is_empty());

        let span = Span::new(Position::new(1, 1), Position::new(1, 2));
        assert!(!span.is_empty());
    }

    #[test]
    fn test_span_contains() {
        let span = Span::new(Position::new(1, 1), Position::new(2, 10));
        assert!(span.contains(Position::new(1, 5)));
        assert!(span.contains(Position::new(2, 5)));
        assert!(!span.contains(Position::new(3, 1)));
        assert!(!span.contains(Position::new(0, 1)));
    }

    #[test]
    fn test_span_default() {
        let span = Span::default();
        assert_eq!(span.start.line, 0);
        assert_eq!(span.end.column, 0);
    }

    #[test]
    fn test_span_to_string() {
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));
        assert_eq!(span.to_string(), "1:1 - 1:10");
    }

    #[test]
    fn test_serde_serialize() {
        let span = Span::new(Position::new(1, 5), Position::new(2, 10));
        let json = serde_json::to_string(&span).expect("Serializing Span should succeed");
        assert!(json.contains("start"));
        assert!(json.contains("end"));
    }

    #[test]
    fn test_serde_deserialize() {
        let json = r#"{"start":{"line":1,"column":5},"end":{"line":2,"column":10}}"#;
        let span: Span = serde_json::from_str(json).expect("deserializing Span should succeed");
        assert_eq!(span.start.line, 1);
        assert_eq!(span.end.column, 10);
    }
}
