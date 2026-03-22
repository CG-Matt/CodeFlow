use anyhow::Result;

use super::{run_sibling_parser_binary, ParseRequest, SourceParser};

#[derive(Debug, Clone)]
pub struct OctaveSourceParser;

impl SourceParser for OctaveSourceParser {
    fn parse_to_graph_json(&self, req: &ParseRequest) -> Result<()> {
        let args: Vec<String> = vec![
            req.input_file.display().to_string(),
            "--output".to_string(),
            req.output_file.display().to_string(),
        ];
        run_sibling_parser_binary("codeflow-extract-octave", &args)
    }
}
