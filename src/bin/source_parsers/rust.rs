use anyhow::Result;

use super::{run_sibling_parser_binary, ParseRequest, SourceParser};

#[derive(Debug, Clone)]
pub struct RustSourceParser;

impl SourceParser for RustSourceParser {
    fn parse_to_graph_json(&self, req: &ParseRequest) -> Result<()> {
        let mut args: Vec<String> = vec![
            req.input_file.display().to_string(),
            "--output".to_string(),
            req.output_file.display().to_string(),
        ];
        if !req.function.is_empty() {
            args.push("--function".to_string());
            args.push(req.function.clone());
        }
        if req.no_compact {
            args.push("--no-compact".to_string());
        }
        if req.compact_max_stmts != 5 {
            args.push("--compact-max-stmts".to_string());
            args.push(req.compact_max_stmts.to_string());
        }
        run_sibling_parser_binary("codeflow-extract-rust", &args)
    }
}
