mod alter_parser;
mod create_parser;
mod describe_parser;
mod drop_parser;
mod helpers;

pub struct DdlParser;

impl DdlParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DdlParser {
    fn default() -> Self {
        Self::new()
    }
}
