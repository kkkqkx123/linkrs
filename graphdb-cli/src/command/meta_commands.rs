use colored::Colorize;

pub fn show_help(topic: Option<&str>) -> String {
    match topic {
        None => show_general_help(),
        Some(t) => show_topic_help(t),
    }
}

fn show_general_help() -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "\n{}\n",
        "GraphDB CLI - Meta Commands".cyan().bold()
    ));
    output.push_str(&"─".repeat(50).dimmed());
    output.push('\n');

    output.push_str(&format!("\n{}\n", "Connection".yellow().bold()));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\connect <space>", "Connect to a graph space"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\disconnect", "Disconnect from current session"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\conninfo", "Display connection information"
    ));

    output.push_str(&format!("\n{}\n", "Object Inspection".yellow().bold()));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\show_spaces  or \\l", "List all graph spaces"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\show_tags   or \\dt", "List all tags (vertex types)"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\show_edges  or \\de", "List all edge types"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\show_indexes or \\di", "List all indexes"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\show_users  or \\du", "List all users"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\show_functions or \\df", "List all functions"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\describe <tag>", "Describe tag structure"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\describe_edge <edge>", "Describe edge type structure"
    ));

    output.push_str(&format!("\n{}\n", "Output Format".yellow().bold()));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\format <fmt>", "Set output format (table, csv, json, vertical, html)"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\pager [cmd]", "Set or disable pager"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\timing", "Toggle query execution time display"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\x", "Toggle expanded/vertical display"
    ));

    output.push_str(&format!("\n{}\n", "Variables".yellow().bold()));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\set [name [value]]", "Set or show variables"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\unset <name>", "Delete a variable"
    ));

    output.push_str(&format!("\n{}\n", "Script and I/O".yellow().bold()));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\i <file>", "Execute commands from file"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\ir <file>", "Execute commands from file (raw, no substitution)"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\o [file]", "Redirect output to file (or close)"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\! <command>", "Execute a shell command"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\import <fmt> <file> <type> <name>", "Import data from file"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\export <fmt> <file> <query> [opts]", "Export query results to file"
    ));
     output.push_str(&format!(
         "  {:25} {}\n",
         "\\copy <target> from|to <file> [opts]", "Copy data to/from file"
     ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\export-space <sp> <path>", "Export full space (by tag/edge-type)"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\export-schema <path>", "Export schema definitions to file"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\import-schema <file>", "Import schema definitions from file"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\dump <db> <path>", "Dump database to directory"
    ));
    output.push_str(&format!(
        "  {:25} {}",
        "\\restore <path> <db>", "Restore database from dump"
    ));

    output.push_str(&format!("\n{}\n", "Query Buffer".yellow().bold()));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\e [file] [+line]", "Edit query buffer in external editor"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\p", "Print the current query buffer"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\r", "Reset (clear) the query buffer"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\w <file>", "Write query buffer to file"
    ));

    output.push_str(&format!("\n{}\n", "History".yellow().bold()));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\history [N]", "Show last N history entries (default 20)"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\history search <pat>", "Search history for pattern"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\history exec <id>", "Re-execute history entry by ID"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\history clear", "Clear command history"
    ));

    output.push_str(&format!("\n{}\n", "Transaction".yellow().bold()));
    output.push_str(&format!("  {:25} {}\n", "\\begin", "Begin a transaction"));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\commit", "Commit current transaction"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\rollback", "Rollback current transaction"
    ));

    output.push_str(&format!("\n{}\n", "Conditional Execution".yellow().bold()));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\if <condition>", "Begin conditional block"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\elif <condition>", "Else-if branch"
    ));
    output.push_str(&format!("  {:25} {}\n", "\\else", "Else branch"));
    output.push_str(&format!("  {:25} {}\n", "\\endif", "End conditional block"));

    output.push_str(&format!("\n{}\n", "General".yellow().bold()));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\help [command]", "Show help on GQL command"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\version", "Show version information"
    ));
    output.push_str(&format!(
        "  {:25} {}\n",
        "\\copyright", "Show copyright information"
    ));
    output.push_str(&format!("  {:25} {}\n", "\\q", "Quit GraphDB CLI"));

    output.push('\n');
    output
}

fn show_topic_help(topic: &str) -> String {
    match topic.to_lowercase().as_str() {
        "match" => {
            let mut s = String::new();
            s.push_str("MATCH statement - Pattern matching query\n\n");
            s.push_str("Syntax:\n");
            s.push_str("  MATCH (v:Tag)\n");
            s.push_str("  WHERE v.property > value\n");
            s.push_str("  RETURN v.property\n\n");
            s.push_str("Examples:\n");
            s.push_str("  MATCH (p:person) RETURN p.name, p.age\n");
            s.push_str("  MATCH (p:person)-[:friend]->(f:person) RETURN p, f\n");
            s
        }
        "go" => {
            let mut s = String::new();
            s.push_str("GO statement - Graph traversal query\n\n");
            s.push_str("Syntax:\n");
            s.push_str("  GO <steps> STEPS FROM <vid> OVER <edge_type>\n");
            s.push_str("  YIELD properties\n\n");
            s.push_str("Examples:\n");
            s.push_str("  GO 1 STEPS FROM \"person1\" OVER friend YIELD friend.name\n");
            s
        }
        "insert" => {
            let mut s = String::new();
            s.push_str("INSERT statement - Insert data\n\n");
            s.push_str("Insert vertex:\n");
            s.push_str("  INSERT VERTEX tag(prop1, prop2) VALUES \"vid\":(val1, val2)\n\n");
            s.push_str("Insert edge:\n");
            s.push_str("  INSERT EDGE edge_type(prop1) VALUES \"src\"->\"dst\":(val1)\n");
            s
        }
        "create" => {
            let mut s = String::new();
            s.push_str("CREATE statement - Schema definition\n\n");
            s.push_str("Create space:\n");
            s.push_str("  CREATE SPACE space_name (vid_type=STRING)\n\n");
            s.push_str("Create tag:\n");
            s.push_str("  CREATE TAG tag_name (prop1 type, prop2 type)\n\n");
            s.push_str("Create edge:\n");
            s.push_str("  CREATE EDGE edge_name (prop1 type)\n");
            s
        }
        "show" => {
            let mut s = String::new();
            s.push_str("SHOW statement - Display metadata\n\n");
            s.push_str("Commands:\n");
            s.push_str("  SHOW SPACES          - List all graph spaces\n");
            s.push_str("  SHOW TAGS            - List all tags in current space\n");
            s.push_str("  SHOW EDGES           - List all edge types in current space\n");
            s.push_str("  SHOW INDEXES         - List all indexes\n");
            s.push_str("  SHOW CREATE TAG <n>  - Show tag creation statement\n");
            s
        }
        "use" => {
            let mut s = String::new();
            s.push_str("USE statement - Switch to a graph space\n\n");
            s.push_str("Syntax:\n");
            s.push_str("  USE space_name\n\n");
            s.push_str("Example:\n");
            s.push_str("  USE my_graph\n");
            s
        }
        "export" => {
            let mut s = String::new();
            s.push_str("Export query results to file\n\n");
            s.push_str("Syntax:\n");
            s.push_str("  \\export <format> <file> <query> [options]\n\n");
            s.push_str("Formats:\n");
            s.push_str("  csv    - CSV format\n");
            s.push_str("  json   - JSON array format\n");
            s.push_str("  jsonl  - JSON Lines format\n\n");
            s.push_str("Options:\n");
            s.push_str("  --stream, -s          - Enable streaming export (memory efficient)\n");
            s.push_str(
                "  --chunk-size <n>, -c  - Set chunk size for streaming (default: 1000)\n\n",
            );
            s.push_str("Examples:\n");
            s.push_str("  \\export csv output.csv 'MATCH (p:person) RETURN p.name, p.age'\n");
            s.push_str("  \\export json output.json 'MATCH (p:person) RETURN p' --stream\n");
            s.push_str("  \\export jsonl output.jsonl 'MATCH (p:person) RETURN p' -s -c 500\n");
            s
        }
        "import" => {
            let mut s = String::new();
            s.push_str("Import data from file\n\n");
            s.push_str("Syntax:\n");
            s.push_str("  \\import <format> <file> <type> <name> [batch_size]\n\n");
            s.push_str("Formats:\n");
            s.push_str("  csv    - CSV format\n");
            s.push_str("  json   - JSON array format\n");
            s.push_str("  jsonl  - JSON Lines format\n\n");
            s.push_str("Types:\n");
            s.push_str("  tag, vertex  - Import as vertices\n");
            s.push_str("  edge         - Import as edges\n\n");
            s.push_str("Examples:\n");
            s.push_str("  \\import csv data.csv tag person 100\n");
            s.push_str("  \\import json data.json vertex person\n");
            s.push_str("  \\import jsonl edges.jsonl edge friend 50\n");
            s
        }
         "copy" => {
             let mut s = String::new();
             s.push_str("Copy data to/from file\n\n");
             s.push_str("Syntax:\n");
             s.push_str("  \\copy <target> from|to <file> [options]\n\n");
             s.push_str("Options:\n");
             s.push_str("  --stream, -s          - Enable streaming export (for 'to' direction)\n");
             s.push_str("  --chunk-size <n>, -c  - Set chunk size for streaming\n\n");
             s.push_str("Examples:\n");
             s.push_str("  \\copy person from 'data.csv'\n");
             s.push_str("  \\copy person to 'output.csv' --stream\n");
             s.push_str("  \\copy person to 'output.json' -s -c 500\n");
             s
         }
          "export-space" => {
              r#"\export-space <space_name> <output_path> [--format csv|json|jsonl] [--tags t1,t2] [--edges e1,e2]

Export a full space's data, organized by tags and edge types.
  --format csv|json|jsonl   Output format (default: csv)
  --tags t1,t2             Export specific tags only (comma-separated)
  --edges e1,e2            Export specific edge types only (comma-separated)

Example:
  \export-space mydb /backup/mydb_export --format csv
  \export-space mydb /backup/mydb_export --tags Person,Company"#.to_string()
          }
          "export-schema" => {
              r#"\export-schema <output_path> [--format json|yaml]

Export schema definitions (tags, edge types) of the current space.
  --format json|yaml       Output format (default: json)

Example:
  \export-schema /backup/mydb_schema.json"#.to_string()
          }
          "import-schema" => {
              r#"\import-schema <file_path>

Import schema definitions from a file.

Example:
  \import-schema /backup/mydb_schema.json"#.to_string()
          }
          "dump" => {
              r#"\dump <database> <output_path> [--format binary|jsonl] [--no-compress]

Dump a database to a directory.
  --format binary|jsonl   Output format (default: binary)
  --no-compress           Disable compression

Example:
  \dump mydb /backup/mydb_dump"#.to_string()
          }
          "restore" => {
              r#"\restore <source_path> <database> [--overwrite] [--strict]

Restore a database from a dump directory.
  --overwrite    Overwrite existing data
  --strict       Strict mode (fail on schema conflicts)

Example:
  \restore /backup/mydb_dump mydb --overwrite"#.to_string()
          }
        "variables" | "set" => {
            let mut s = String::new();
            s.push_str("Variable Management\n\n");
            s.push_str("Set a variable:\n");
            s.push_str("  \\set NAME VALUE\n\n");
            s.push_str("Show a variable:\n");
            s.push_str("  \\set NAME\n\n");
            s.push_str("Show all variables:\n");
            s.push_str("  \\set\n\n");
            s.push_str("Delete a variable:\n");
            s.push_str("  \\unset NAME\n\n");
            s.push_str("Use variables in queries:\n");
            s.push_str("  MATCH (p:person) WHERE p.age > :min_age RETURN p\n");
            s.push_str("  MATCH (p:person) WHERE p.name = :'name' RETURN p\n\n");
            s.push_str("Special variables (marked with *):\n");
            s.push_str("  ON_ERROR_STOP  - Stop on error (on/off)\n");
            s.push_str("  ECHO           - Echo mode (none/queries/all)\n");
            s.push_str("  TIMING         - Show execution time (on/off)\n");
            s.push_str("  EDITOR         - External editor command\n");
            s.push_str("  FORMAT         - Output format\n");
            s.push_str("  HISTSIZE       - Max history entries\n");
            s.push_str("  AUTOCOMMIT     - Auto-commit mode (on/off)\n");
            s
        }
        "if" | "conditional" => {
            let mut s = String::new();
            s.push_str("Conditional Execution\n\n");
            s.push_str("Syntax:\n");
            s.push_str("  \\if <condition>\n");
            s.push_str("    <commands>\n");
            s.push_str("  \\elif <condition>\n");
            s.push_str("    <commands>\n");
            s.push_str("  \\else\n");
            s.push_str("    <commands>\n");
            s.push_str("  \\endif\n\n");
            s.push_str("Conditions:\n");
            s.push_str("  VAR          - True if variable is set\n");
            s.push_str("  ?VAR         - True if variable is set\n");
            s.push_str("  !?VAR        - True if variable is not set\n");
            s.push_str("  VAR == VALUE - True if variable equals value\n");
            s.push_str("  VAR != VALUE - True if variable not equals value\n\n");
            s.push_str("Example:\n");
            s.push_str("  \\set mode test\n");
            s.push_str("  \\if mode == test\n");
            s.push_str("    MATCH (p:person) RETURN p LIMIT 10;\n");
            s.push_str("  \\else\n");
            s.push_str("    MATCH (p:person) RETURN p;\n");
            s.push_str("  \\endif\n");
            s
        }
        "history" => {
            let mut s = String::new();
            s.push_str("Command History\n\n");
            s.push_str("Commands:\n");
            s.push_str("  \\history [N]          - Show last N entries (default 20)\n");
            s.push_str("  \\history search <pat> - Search history for pattern\n");
            s.push_str("  \\history exec <id>    - Re-execute entry by ID\n");
            s.push_str("  \\history clear        - Clear all history\n\n");
            s.push_str("History is saved to ~/.graphdb/cli_history\n");
            s.push_str("Use UP/DOWN arrows to navigate history in the REPL.\n");
            s
        }
        "edit" | "buffer" => {
            let mut s = String::new();
            s.push_str("Query Buffer and External Editor\n\n");
            s.push_str("Commands:\n");
            s.push_str("  \\e [file] [+line]  - Edit in external editor\n");
            s.push_str("  \\p                  - Print current buffer\n");
            s.push_str("  \\r                  - Reset (clear) buffer\n");
            s.push_str("  \\w <file>           - Write buffer to file\n\n");
            s.push_str("The editor is determined by:\n");
            s.push_str("  1. \\set EDITOR <cmd>\n");
            s.push_str("  2. EDITOR environment variable\n");
            s.push_str("  3. VISUAL environment variable\n");
            s.push_str("  4. Default: vi (or notepad on Windows)\n");
            s
        }
        _ => format!(
            "No help available for '{}'. Type \\? for a list of meta-commands.",
            topic
        ),
    }
}

pub fn show_version() -> String {
    format!(
        "GraphDB CLI v{}\nGraphDB - A lightweight single-node graph database",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn show_copyright() -> String {
    "GraphDB CLI\n\
     Copyright (c) 2024 GraphDB Contributors\n\
     Licensed under the Apache License, Version 2.0"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_show_help_general() {
        let help = show_help(None);
        assert!(help.contains("GraphDB CLI"));
        assert!(help.contains("Connection"));
        assert!(help.contains("Object Inspection"));
        assert!(help.contains("Output Format"));
        assert!(help.contains("Variables"));
        assert!(help.contains("Script and I/O"));
        assert!(help.contains("Query Buffer"));
        assert!(help.contains("History"));
        assert!(help.contains("Transaction"));
        assert!(help.contains("Conditional Execution"));
        assert!(help.contains("General"));
    }

    #[test]
    fn test_show_help_match() {
        let help = show_help(Some("match"));
        assert!(help.contains("MATCH"));
        assert!(help.contains("Pattern matching"));
    }

    #[test]
    fn test_show_help_go() {
        let help = show_help(Some("go"));
        assert!(help.contains("GO"));
        assert!(help.contains("traversal"));
    }

    #[test]
    fn test_show_help_insert() {
        let help = show_help(Some("insert"));
        assert!(help.contains("INSERT"));
        assert!(help.contains("vertex"));
        assert!(help.contains("edge"));
    }

    #[test]
    fn test_show_help_create() {
        let help = show_help(Some("create"));
        assert!(help.contains("CREATE"));
        assert!(help.contains("space"));
        assert!(help.contains("tag"));
    }

    #[test]
    fn test_show_help_show() {
        let help = show_help(Some("show"));
        assert!(help.contains("SHOW"));
        assert!(help.contains("SPACES"));
        assert!(help.contains("TAGS"));
    }

    #[test]
    fn test_show_help_use() {
        let help = show_help(Some("use"));
        assert!(help.contains("USE"));
        assert!(help.contains("space"));
    }

    #[test]
    fn test_show_help_variables() {
        let help = show_help(Some("variables"));
        assert!(help.contains("Variable"));
        assert!(help.contains("\\set"));
        assert!(help.contains("\\unset"));
    }

    #[test]
    fn test_show_help_set() {
        let help = show_help(Some("set"));
        assert!(help.contains("Variable"));
    }

    #[test]
    fn test_show_help_if() {
        let help = show_help(Some("if"));
        assert!(help.contains("Conditional"));
        assert!(help.contains("\\if"));
        assert!(help.contains("\\elif"));
        assert!(help.contains("\\else"));
        assert!(help.contains("\\endif"));
    }

    #[test]
    fn test_show_help_conditional() {
        let help = show_help(Some("conditional"));
        assert!(help.contains("Conditional"));
    }

    #[test]
    fn test_show_help_history() {
        let help = show_help(Some("history"));
        assert!(help.contains("History"));
        assert!(help.contains("\\history"));
    }

    #[test]
    fn test_show_help_edit() {
        let help = show_help(Some("edit"));
        assert!(help.contains("Buffer"));
        assert!(help.contains("\\e"));
    }

    #[test]
    fn test_show_help_buffer() {
        let help = show_help(Some("buffer"));
        assert!(help.contains("Buffer"));
    }

    #[test]
    fn test_show_help_unknown() {
        let help = show_help(Some("unknown_topic"));
        assert!(help.contains("No help available"));
        assert!(help.contains("unknown_topic"));
    }

    #[test]
    fn test_show_help_case_insensitive() {
        let help_lower = show_help(Some("match"));
        let help_upper = show_help(Some("MATCH"));
        let help_mixed = show_help(Some("Match"));
        assert_eq!(help_lower, help_upper);
        assert_eq!(help_lower, help_mixed);
    }

    #[test]
    fn test_show_version() {
        let version = show_version();
        assert!(version.contains("GraphDB CLI"));
        assert!(version.contains("v"));
        assert!(version.contains("graph database"));
    }

    #[test]
    fn test_show_copyright() {
        let copyright = show_copyright();
        assert!(copyright.contains("GraphDB CLI"));
        assert!(copyright.contains("Copyright"));
        assert!(copyright.contains("Apache License"));
    }

    #[test]
    fn test_general_help_contains_commands() {
        let help = show_general_help();

        assert!(help.contains("\\connect"));
        assert!(help.contains("\\disconnect"));
        assert!(help.contains("\\conninfo"));
        assert!(help.contains("\\show_spaces") || help.contains("\\l"));
        assert!(help.contains("\\show_tags") || help.contains("\\dt"));
        assert!(help.contains("\\show_edges") || help.contains("\\de"));
        assert!(help.contains("\\show_indexes") || help.contains("\\di"));
        assert!(help.contains("\\show_users") || help.contains("\\du"));
        assert!(help.contains("\\show_functions") || help.contains("\\df"));
        assert!(help.contains("\\describe") || help.contains("\\d"));
        assert!(help.contains("\\format"));
        assert!(help.contains("\\pager"));
        assert!(help.contains("\\timing"));
        assert!(help.contains("\\x"));
        assert!(help.contains("\\set"));
        assert!(help.contains("\\unset"));
        assert!(help.contains("\\i "));
        assert!(help.contains("\\ir"));
        assert!(help.contains("\\o"));
        assert!(help.contains("\\!"));
        assert!(help.contains("\\e") || help.contains("\\edit"));
        assert!(help.contains("\\p"));
        assert!(help.contains("\\r"));
        assert!(help.contains("\\w"));
        assert!(help.contains("\\history"));
        assert!(help.contains("\\begin"));
        assert!(help.contains("\\commit"));
        assert!(help.contains("\\rollback"));
        assert!(help.contains("\\if"));
        assert!(help.contains("\\elif"));
        assert!(help.contains("\\else"));
        assert!(help.contains("\\endif"));
        assert!(help.contains("\\help"));
        assert!(help.contains("\\version"));
        assert!(help.contains("\\copyright"));
        assert!(help.contains("\\q"));
    }
}
