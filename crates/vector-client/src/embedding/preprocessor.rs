use serde::{Deserialize, Serialize};

/// Preprocessor configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PreprocessorConfig {
    /// No preprocessing (default)
    #[default]
    None,
    /// Simple prefix
    Prefix { prefix: String },
    /// Template with {text} placeholder
    Template { template: String },
    /// Nomic-Embed task type
    Nomic { task_type: NomicTaskType },
    /// Stella task type
    Stella { task_type: StellaTaskType },
}

/// Concrete preprocessor implementation — no trait object needed
#[derive(Debug, Clone)]
pub enum PreprocessorImpl {
    None,
    Prefix { prefix: String },
    Template { template: String },
    Nomic { task_type: NomicTaskType },
    Stella { task_type: StellaTaskType },
}

impl PreprocessorImpl {
    pub fn from_config(config: &PreprocessorConfig) -> Self {
        match config {
            PreprocessorConfig::None => Self::None,
            PreprocessorConfig::Prefix { prefix } => Self::Prefix {
                prefix: prefix.clone(),
            },
            PreprocessorConfig::Template { template } => Self::Template {
                template: template.clone(),
            },
            PreprocessorConfig::Nomic { task_type } => Self::Nomic {
                task_type: *task_type,
            },
            PreprocessorConfig::Stella { task_type } => Self::Stella {
                task_type: *task_type,
            },
        }
    }

    pub fn preprocess(&self, text: &str) -> String {
        match self {
            Self::None => text.to_string(),
            Self::Prefix { prefix } => format!("{}{}", prefix, text),
            Self::Template { template } => template.replace("{{text}}", text),
            Self::Nomic { task_type } => {
                let prefix = match task_type {
                    NomicTaskType::SearchQuery => "search_query: ",
                    NomicTaskType::SearchDocument => "search_document: ",
                    NomicTaskType::Classification => "classification: ",
                    NomicTaskType::Clustering => "clustering: ",
                };
                format!("{}{}", prefix, text)
            }
            Self::Stella { task_type } => {
                let prefix = match task_type {
                    StellaTaskType::S2PQuery => {
                        "Instruct: Given a web search query, retrieve relevant passages. Query: "
                    }
                    StellaTaskType::S2SDocument => {
                        "Instruct: Given a web search query, retrieve relevant passages. Document: "
                    }
                    StellaTaskType::P2PQuery => {
                        "Instruct: Given a passage, retrieve relevant passages. Query: "
                    }
                    StellaTaskType::P2PDocument => {
                        "Instruct: Given a passage, retrieve relevant passages. Document: "
                    }
                };
                format!("{}{}", prefix, text)
            }
        }
    }

    pub fn process_batch(&self, texts: &[&str]) -> Vec<String> {
        texts.iter().map(|&t| self.preprocess(t)).collect()
    }
}

/// Nomic task type
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NomicTaskType {
    SearchQuery,
    SearchDocument,
    Classification,
    Clustering,
}

/// Stella task type
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum StellaTaskType {
    S2PQuery,
    S2SDocument,
    P2PQuery,
    P2PDocument,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_preprocessor() {
        let p = PreprocessorImpl::None;
        assert_eq!(p.preprocess("hello world"), "hello world");
    }

    #[test]
    fn test_noop_preprocessor_empty() {
        let p = PreprocessorImpl::None;
        assert_eq!(p.preprocess(""), "");
    }

    #[test]
    fn test_prefix_preprocessor() {
        let p = PreprocessorImpl::Prefix {
            prefix: "query: ".into(),
        };
        assert_eq!(p.preprocess("rust"), "query: rust");
    }

    #[test]
    fn test_prefix_preprocessor_no_space() {
        let p = PreprocessorImpl::Prefix {
            prefix: "cls:".into(),
        };
        assert_eq!(p.preprocess("text"), "cls:text");
    }

    #[test]
    fn test_template_preprocessor() {
        let p = PreprocessorImpl::Template {
            template: "classify: {{text}}".into(),
        };
        assert_eq!(p.preprocess("hello"), "classify: hello");
    }

    #[test]
    fn test_template_preprocessor_multiple_placeholders() {
        let p = PreprocessorImpl::Template {
            template: "{{text}} and {{text}}".into(),
        };
        assert_eq!(p.preprocess("x"), "x and x");
    }

    #[test]
    fn test_nomic_preprocessor_search_query() {
        let p = PreprocessorImpl::Nomic {
            task_type: NomicTaskType::SearchQuery,
        };
        assert_eq!(p.preprocess("rust"), "search_query: rust");
    }

    #[test]
    fn test_nomic_preprocessor_search_document() {
        let p = PreprocessorImpl::Nomic {
            task_type: NomicTaskType::SearchDocument,
        };
        assert_eq!(p.preprocess("doc"), "search_document: doc");
    }

    #[test]
    fn test_nomic_preprocessor_classification() {
        let p = PreprocessorImpl::Nomic {
            task_type: NomicTaskType::Classification,
        };
        assert_eq!(p.preprocess("text"), "classification: text");
    }

    #[test]
    fn test_nomic_preprocessor_clustering() {
        let p = PreprocessorImpl::Nomic {
            task_type: NomicTaskType::Clustering,
        };
        assert_eq!(p.preprocess("data"), "clustering: data");
    }

    #[test]
    fn test_stella_preprocessor_s2p_query() {
        let p = PreprocessorImpl::Stella {
            task_type: StellaTaskType::S2PQuery,
        };
        assert!(p.preprocess("test").contains("web search query"));
        assert!(p.preprocess("test").contains("test"));
    }

    #[test]
    fn test_stella_preprocessor_s2s_document() {
        let p = PreprocessorImpl::Stella {
            task_type: StellaTaskType::S2SDocument,
        };
        assert!(p.preprocess("doc").contains("Document:"));
    }

    #[test]
    fn test_stella_preprocessor_p2p_query() {
        let p = PreprocessorImpl::Stella {
            task_type: StellaTaskType::P2PQuery,
        };
        assert!(p.preprocess("q").contains("passage"));
        assert!(p.preprocess("q").contains("q"));
    }

    #[test]
    fn test_stella_preprocessor_p2p_document() {
        let p = PreprocessorImpl::Stella {
            task_type: StellaTaskType::P2PDocument,
        };
        assert!(p.preprocess("d").contains("Document:"));
    }

    #[test]
    fn test_from_config_none() {
        let p = PreprocessorImpl::from_config(&PreprocessorConfig::None);
        assert_eq!(p.preprocess("hello"), "hello");
    }

    #[test]
    fn test_from_config_prefix() {
        let p = PreprocessorImpl::from_config(&PreprocessorConfig::Prefix {
            prefix: ">> ".into(),
        });
        assert_eq!(p.preprocess("x"), ">> x");
    }

    #[test]
    fn test_from_config_template() {
        let p = PreprocessorImpl::from_config(&PreprocessorConfig::Template {
            template: "[{{text}}]".into(),
        });
        assert_eq!(p.preprocess("x"), "[x]");
    }

    #[test]
    fn test_batch_processing() {
        let p = PreprocessorImpl::Prefix {
            prefix: "> ".into(),
        };
        let texts = vec!["a", "b", "c"];
        let result = p.process_batch(&texts);
        assert_eq!(result, vec!["> a", "> b", "> c"]);
    }

    #[test]
    fn test_from_config_nomic() {
        let p = PreprocessorImpl::from_config(&PreprocessorConfig::Nomic {
            task_type: NomicTaskType::SearchQuery,
        });
        assert_eq!(p.preprocess("hello"), "search_query: hello");
    }

    #[test]
    fn test_from_config_stella() {
        let p = PreprocessorImpl::from_config(&PreprocessorConfig::Stella {
            task_type: StellaTaskType::S2PQuery,
        });
        assert!(p.preprocess("hello").contains("Instruct"));
    }
}
