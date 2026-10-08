pub struct StatementBalanceTracker {
    in_single_line_comment: bool,
    in_multi_line_comment: bool,
    in_single_quote: bool,
    in_double_quote: bool,
    paren_depth: i32,
    brace_depth: i32,
    bracket_depth: i32,
    prev_char: Option<char>,
}

impl StatementBalanceTracker {
    pub fn new() -> Self {
        Self {
            in_single_line_comment: false,
            in_multi_line_comment: false,
            in_single_quote: false,
            in_double_quote: false,
            paren_depth: 0,
            brace_depth: 0,
            bracket_depth: 0,
            prev_char: None,
        }
    }

    pub fn feed(&mut self, ch: char) {
        if self.in_single_line_comment {
            if ch == '\n' {
                self.in_single_line_comment = false;
            }
            self.prev_char = Some(ch);
            return;
        }

        if self.in_multi_line_comment {
            if ch == '/' && self.prev_char == Some('*') {
                self.in_multi_line_comment = false;
            }
            self.prev_char = Some(ch);
            return;
        }

        if !self.in_single_quote && !self.in_double_quote {
            if ch == '-' && self.prev_char == Some('-') {
                self.in_single_line_comment = true;
                self.prev_char = Some(ch);
                return;
            }
            if ch == '*' && self.prev_char == Some('/') {
                self.in_multi_line_comment = true;
                self.prev_char = Some(ch);
                return;
            }
        }

        match ch {
            '\'' if !self.in_double_quote => {
                self.in_single_quote = !self.in_single_quote;
            }
            '"' if !self.in_single_quote => {
                self.in_double_quote = !self.in_double_quote;
            }
            '(' if !self.in_any_string() => self.paren_depth += 1,
            ')' if !self.in_any_string() => self.paren_depth = (self.paren_depth - 1).max(0),
            '{' if !self.in_any_string() => self.brace_depth += 1,
            '}' if !self.in_any_string() => self.brace_depth = (self.brace_depth - 1).max(0),
            '[' if !self.in_any_string() => self.bracket_depth += 1,
            ']' if !self.in_any_string() => self.bracket_depth = (self.bracket_depth - 1).max(0),
            _ => {}
        }

        self.prev_char = Some(ch);
    }

    pub fn is_balanced(&self) -> bool {
        !self.in_single_quote
            && !self.in_double_quote
            && !self.in_multi_line_comment
            && self.paren_depth == 0
            && self.brace_depth == 0
            && self.bracket_depth == 0
    }

    fn in_any_string(&self) -> bool {
        self.in_single_quote || self.in_double_quote
    }
}

impl Default for StatementBalanceTracker {
    fn default() -> Self {
        Self::new()
    }
}
