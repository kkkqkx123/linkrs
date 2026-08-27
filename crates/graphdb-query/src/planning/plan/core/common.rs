//! Definition of the General Plan Node Structure

// Tag attribute structure
#[derive(Debug, Clone)]
pub struct TagProp {
    pub tag: String,
    pub props: Vec<String>,
}

impl TagProp {
    pub fn new(tag: &str, props: Vec<String>) -> Self {
        Self {
            tag: tag.to_string(),
            props,
        }
    }
}

// Edge Attribute Structure
#[derive(Debug, Clone)]
pub struct EdgeProp {
    pub edge_type: String,
    pub props: Vec<String>,
}

impl EdgeProp {
    pub fn new(edge_type: &str, props: Vec<String>) -> Self {
        Self {
            edge_type: edge_type.to_string(),
            props,
        }
    }
}
