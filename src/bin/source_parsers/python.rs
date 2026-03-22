use anyhow::Result;

use super::{run_sibling_parser_binary, ParseRequest, SourceParser};

#[derive(Debug, Clone)]
pub struct PythonSourceParser;

impl SourceParser for PythonSourceParser {
    fn parse_to_graph_json(&self, req: &ParseRequest) -> Result<()> {
        let mut args: Vec<String> = vec![
            req.input_file.display().to_string(),
            "--output".to_string(),
            req.output_file.display().to_string(),
            "--function".to_string(),
            req.function.clone(),
            "--compact-max-stmts".to_string(),
            req.compact_max_stmts.to_string(),
        ];
        if req.no_compact {
            args.push("--no-compact".to_string());
        }
        run_sibling_parser_binary("codeflow-extract-python", &args)
    }
}
