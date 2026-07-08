use jieba_rs::{Jieba, TokenizeMode};
use parking_lot::Mutex;
use tantivy_tokenizer_api::{Token, TokenStream, Tokenizer};

pub struct JiebaTokenizer {
    jieba: Mutex<Jieba>,
}

impl Clone for JiebaTokenizer {
    fn clone(&self) -> Self {
        Self {
            jieba: Mutex::new(Jieba::new()),
        }
    }
}

impl Default for JiebaTokenizer {
    fn default() -> Self {
        Self {
            jieba: Mutex::new(Jieba::new()),
        }
    }
}

impl Tokenizer for JiebaTokenizer {
    type TokenStream<'a> = JiebaTokenStream<'a>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        let jieba = self.jieba.lock();
        let tokens = jieba.tokenize(text, TokenizeMode::Search, true);
        JiebaTokenStream::new(text, tokens)
    }
}

pub struct JiebaTokenStream<'a> {
    tokens: Vec<jieba_rs::Token<'a>>,
    char_offsets: Vec<usize>,
    token: Token,
    index: usize,
}

impl<'a> JiebaTokenStream<'a> {
    fn new(text: &'a str, tokens: Vec<jieba_rs::Token<'a>>) -> Self {
        Self {
            tokens,
            char_offsets: char_offsets(text),
            token: Token::default(),
            index: 0,
        }
    }
}

impl TokenStream for JiebaTokenStream<'_> {
    fn advance(&mut self) -> bool {
        let Some(token) = self.tokens.get(self.index) else {
            return false;
        };
        self.index += 1;

        self.token.position = self.token.position.wrapping_add(1);
        self.token.offset_from = self.byte_offset(token.start);
        self.token.offset_to = self.byte_offset(token.end);
        self.token.text.clear();
        self.token.text.push_str(token.word);
        self.token.position_length = 1;
        true
    }

    fn token(&self) -> &Token {
        &self.token
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.token
    }
}

impl JiebaTokenStream<'_> {
    fn byte_offset(&self, char_index: usize) -> usize {
        self.char_offsets
            .get(char_index)
            .copied()
            .unwrap_or_else(|| self.char_offsets.last().copied().unwrap_or(0))
    }
}

fn char_offsets(text: &str) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(text.chars().count() + 1);
    offsets.push(0);
    for (byte_index, _) in text.char_indices().skip(1) {
        offsets.push(byte_index);
    }
    offsets.push(text.len());
    offsets
}

#[cfg(test)]
mod tests {
    use super::JiebaTokenizer;
    use tantivy_tokenizer_api::{Token, TokenStream, Tokenizer};

    #[test]
    fn tokenizes_ascii_words() {
        let mut tokenizer = JiebaTokenizer::default();
        let mut stream = tokenizer.token_stream("hello world");
        let mut tokens: Vec<Token> = Vec::new();
        let mut collect = |token: &Token| tokens.push(token.clone());
        stream.process(&mut collect);

        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].text, "hello");
        assert_eq!(tokens[0].offset_from, 0);
        assert_eq!(tokens[0].offset_to, 5);
        assert_eq!(tokens[1].text, " ");
        assert_eq!(tokens[1].offset_from, 5);
        assert_eq!(tokens[1].offset_to, 6);
        assert_eq!(tokens[2].text, "world");
        assert_eq!(tokens[2].offset_from, 6);
        assert_eq!(tokens[2].offset_to, 11);
    }

    #[test]
    fn tokenizes_chinese_text() {
        let mut tokenizer = JiebaTokenizer::default();
        let mut stream = tokenizer.token_stream("中华人民共和国");
        let mut tokens: Vec<Token> = Vec::new();
        let mut collect = |token: &Token| tokens.push(token.clone());
        stream.process(&mut collect);

        assert!(tokens.len() > 1);
        assert_eq!(tokens[0].text, "中华");
        assert_eq!(tokens[0].offset_from, 0);
        assert_eq!(tokens[0].offset_to, "中华".len());
    }
}
