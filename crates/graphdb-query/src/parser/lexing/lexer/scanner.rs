//! Character-level scanning for the lexer.

use crate::parser::lexing::LexError;

use super::Lexer;

impl<'a> Lexer<'a> {
    pub(super) fn skip_whitespace(&mut self) {
        while let Some(&ch) = self.peek_char() {
            if ch == ' ' || ch == '\t' || ch == '\r' || ch == '\n' {
                self.read_char();
            } else {
                break;
            }
        }
    }

    pub(super) fn read_identifier(&mut self) -> String {
        let start = self.position;
        while let Some(&ch) = self.peek_char() {
            if ch.is_alphanumeric() || ch == '_' {
                self.read_char();
            } else {
                break;
            }
        }
        self.input
            .get(start..self.position)
            .unwrap_or("")
            .to_string()
    }

    pub(super) fn read_backtick_identifier(&mut self) -> String {
        let start = self.position;
        loop {
            match self.peek_char() {
                Some(&'`') => {
                    self.read_char();
                    break;
                }
                Some(&ch) if ch == '\n' || ch == '\r' => {
                    break;
                }
                Some(_) => {
                    self.read_char();
                }
                None => break,
            }
        }
        self.input
            .get(start..self.position.saturating_sub(1))
            .unwrap_or("")
            .to_string()
    }

    pub(super) fn read_number(&mut self) -> String {
        let start = self.position;
        let mut has_decimal = false;
        let mut has_exponent = false;

        while let Some(&ch) = self.peek_char() {
            if ch.is_ascii_digit() {
                self.read_char();
            } else if ch == '.' && !has_decimal && !has_exponent {
                // Check whether a number follows the text (using “peek” without consuming any characters).
                let mut temp_chars = self.chars.clone();
                temp_chars.next(); // Skip.
                if temp_chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                    has_decimal = true;
                    self.read_char();
                } else {
                    break;
                }
            } else if (ch == 'e' || ch == 'E') && !has_exponent {
                has_exponent = true;
                self.read_char();
                if let Some(&ch) = self.peek_char() {
                    if ch == '+' || ch == '-' {
                        self.read_char();
                    }
                }
            } else {
                break;
            }
        }
        self.input
            .get(start..self.position)
            .unwrap_or("")
            .to_string()
    }

    pub(super) fn read_string(&mut self) -> Result<String, LexError> {
        let start_position = self.current_position();
        let quote = match self.read_char() {
            Some(ch) => ch,
            None => {
                return Err(LexError::new(
                    "Unexpected end of input while reading string".to_string(),
                    start_position,
                ));
            }
        };

        let mut result = String::new();

        loop {
            match self.peek_char() {
                Some(&'\\') => {
                    self.read_char();
                    if let Some(ch) = self.read_char() {
                        match ch {
                            'n' => result.push('\n'),
                            't' => result.push('\t'),
                            'r' => result.push('\r'),
                            '\\' => result.push('\\'),
                            '"' => result.push('"'),
                            '\'' => result.push('\''),
                            '`' => result.push('`'),
                            '0' => result.push('\0'),
                            '\n' => {
                                while let Some(&c) = self.peek_char() {
                                    if c == ' ' || c == '\t' {
                                        self.read_char();
                                    } else {
                                        break;
                                    }
                                }
                                continue;
                            }
                            'u' => {
                                let mut unicode_seq = String::new();
                                for _ in 0..4 {
                                    if let Some(&c) = self.peek_char() {
                                        if c.is_ascii_hexdigit() {
                                            unicode_seq.push(c);
                                            self.read_char();
                                        } else {
                                            break;
                                        }
                                    } else {
                                        break;
                                    }
                                }
                                if !unicode_seq.is_empty() {
                                    if let Ok(code_point) = u32::from_str_radix(&unicode_seq, 16) {
                                        if let Some(ch) = char::from_u32(code_point) {
                                            result.push(ch);
                                        }
                                    } else {
                                        self.add_error(LexError::invalid_escape_sequence(
                                            format!("u{}", unicode_seq),
                                            self.current_position(),
                                        ));
                                    }
                                }
                            }
                            'x' => {
                                let mut hex_seq = String::new();
                                for _ in 0..2 {
                                    if let Some(&c) = self.peek_char() {
                                        if c.is_ascii_hexdigit() {
                                            hex_seq.push(c);
                                            self.read_char();
                                        } else {
                                            break;
                                        }
                                    } else {
                                        break;
                                    }
                                }
                                if !hex_seq.is_empty() {
                                    if let Ok(byte) = u8::from_str_radix(&hex_seq, 16) {
                                        result.push(byte as char);
                                    } else {
                                        self.add_error(LexError::invalid_escape_sequence(
                                            format!("x{}", hex_seq),
                                            self.current_position(),
                                        ));
                                    }
                                }
                            }
                            _ => {
                                result.push('\\');
                                result.push(ch);
                            }
                        }
                    }
                }
                Some(&'\'') | Some(&'"') => match self.peek_char() {
                    Some(&q) if q == quote => {
                        self.read_char();
                        return Ok(result);
                    }
                    Some(_) => {
                        let ch = self.read_char().ok_or_else(|| {
                            LexError::unexpected_end_of_input(self.current_position())
                        })?;
                        result.push(ch);
                    }
                    None => {
                        self.add_error(LexError::unterminated_string(start_position));
                        return Err(LexError::unterminated_string(start_position));
                    }
                },
                Some(&'\n') => {
                    self.add_error(LexError::unterminated_string(start_position));
                    return Err(LexError::unterminated_string(start_position));
                }
                Some(&ch) => {
                    result.push(ch);
                    self.read_char();
                }
                None => {
                    self.add_error(LexError::unterminated_string(start_position));
                    return Err(LexError::unterminated_string(start_position));
                }
            }
        }
    }
    pub(super) fn skip_comment(&mut self) -> Result<(), LexError> {
        let start_position = self.current_position();

        match self.peek_char() {
            Some(&'/') => {
                // Use clone to peek ahead without consuming characters
                let mut temp_chars = self.chars.clone();
                temp_chars.next(); // Skip the first /
                match temp_chars.peek() {
                    Some(&'/') => {
                        // Line comment: // ...
                        self.read_char(); // consume /
                        self.read_char(); // consume /
                        while let Some(&ch) = self.peek_char() {
                            if ch == '\n' {
                                break;
                            }
                            self.read_char();
                        }
                        Ok(())
                    }
                    Some(&'*') => {
                        // Block comment: /* ... */
                        self.read_char(); // consume /
                        self.read_char(); // consume *
                        loop {
                            match self.peek_char() {
                                Some(&'*') => {
                                    self.read_char();
                                    if let Some(&'/') = self.peek_char() {
                                        self.read_char();
                                        return Ok(());
                                    }
                                }
                                Some(&'\n') => {
                                    self.read_char();
                                    return Ok(());
                                }
                                Some(_) => {
                                    self.read_char();
                                }
                                None => {
                                    let error = LexError::unterminated_comment(start_position);
                                    self.add_error(error.clone());
                                    return Err(error);
                                }
                            }
                        }
                    }
                    _ => {
                        // Not a comment (e.g., / is division operator)
                        Err(LexError::new(
                            "Not a comment".to_string(),
                            self.current_position(),
                        ))
                    }
                }
            }
            Some(&'-') => {
                // Check whether the next character is also "-" (use the "clone" function to avoid consuming the current character).
                let mut temp_chars = self.chars.clone();
                temp_chars.next(); // Skip the first one.
                if let Some(&'-') = temp_chars.peek() {
                    // These are SQL comments; they consume two "-" characters each.
                    self.read_char(); // Read the first one -
                    self.read_char(); // Read the second one...
                    while let Some(&ch) = self.peek_char() {
                        if ch == '\n' {
                            break;
                        }
                        self.read_char();
                    }
                    Ok(())
                } else {
                    // Don't return errors; let the caller handle them.
                    Err(LexError::new(
                        "Not a comment".to_string(),
                        self.current_position(),
                    ))
                }
            }
            _ => Ok(()),
        }
    }
    /// Parse vector elements after '[' in VECTOR[...] syntax
    pub(super) fn parse_vector_elements(&mut self) -> Vec<f32> {
        let mut elements = Vec::new();

        loop {
            self.skip_whitespace();

            // Parse number
            if let Some(&ch) = self.peek_char() {
                if ch.is_ascii_digit() || ch == '-' || ch == '.' {
                    let number_str = self.read_number();
                    if let Ok(num) = number_str.parse::<f32>() {
                        elements.push(num);
                    }
                }
            }

            self.skip_whitespace();

            // Check for comma or end
            if let Some(&',') = self.peek_char() {
                self.read_char(); // consume comma
            } else {
                break;
            }
        }

        // consume ']'
        if let Some(&']') = self.peek_char() {
            self.read_char();
        }

        elements
    }
}
