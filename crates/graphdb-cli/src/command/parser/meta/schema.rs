use crate::command::parser::types::MetaCommand;

pub fn parse_show_tags(arg: &str) -> Result<MetaCommand, String> {
    let pattern = if arg.is_empty() {
        None
    } else {
        Some(arg.to_string())
    };
    Ok(MetaCommand::ShowTags { pattern })
}

pub fn parse_show_edges(arg: &str) -> Result<MetaCommand, String> {
    let pattern = if arg.is_empty() {
        None
    } else {
        Some(arg.to_string())
    };
    Ok(MetaCommand::ShowEdges { pattern })
}

pub fn parse_show_indexes(arg: &str) -> Result<MetaCommand, String> {
    let pattern = if arg.is_empty() {
        None
    } else {
        Some(arg.to_string())
    };
    Ok(MetaCommand::ShowIndexes { pattern })
}

pub fn parse_describe(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\describe <tag_name>".to_string())
    } else {
        Ok(MetaCommand::Describe {
            object: arg.to_string(),
        })
    }
}

pub fn parse_describe_edge(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\describe_edge <edge_name>".to_string())
    } else {
        Ok(MetaCommand::DescribeEdge {
            name: arg.to_string(),
        })
    }
}
