use text_size::TextSize;

/// A 1-based line and column. Columns count characters, not bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

/// Maps byte offsets in a source text to [`LineCol`] positions.
pub struct LineIndex<'a> {
    text: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub fn new(text: &'a str) -> Self {
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        LineIndex { text, line_starts }
    }

    pub fn line_col(&self, offset: TextSize) -> LineCol {
        let offset = usize::from(offset).min(self.text.len());
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let col = self.text[self.line_starts[line]..offset].chars().count();
        LineCol {
            line: line as u32 + 1,
            col: col as u32 + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_lines_and_characters() {
        let index = LineIndex::new("ab\nçd\n");
        assert_eq!(index.line_col(0.into()), LineCol { line: 1, col: 1 });
        assert_eq!(index.line_col(3.into()), LineCol { line: 2, col: 1 });
        // 'ç' is two bytes, so byte 5 is the second character of line 2.
        assert_eq!(index.line_col(5.into()), LineCol { line: 2, col: 2 });
        assert_eq!(index.line_col(7.into()), LineCol { line: 3, col: 1 });
    }
}
