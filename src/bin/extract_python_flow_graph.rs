use std::cmp::max;
use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(name = "codeflow-extract-python")]
#[command(about = "CodeFlow Python parser (indentation-based).")]
struct Cli {
    input_file: PathBuf,

    #[arg(
        long,
        default_value = "main",
        help = "Function name to parse (default: main). Use '' for module body."
    )]
    function: String,

    #[arg(short, long)]
    output: Option<PathBuf>,

    #[arg(long, help = "Disable process-block compaction.")]
    no_compact: bool,

    #[arg(
        long,
        default_value_t = 5,
        help = "Max linear process statements merged into one block."
    )]
    compact_max_stmts: usize,
}

#[derive(Debug, Clone)]
struct Exit {
    node_id: String,
    label: Option<String>,
}

#[derive(Debug, Clone)]
struct LineInfo {
    lineno: usize,
    line_end: usize,
    indent: usize,
    code: String,
}

#[derive(Debug, Clone, Serialize)]
struct Node {
    id: String,
    shape: String,
    text: String,
    line_start: Option<usize>,
    line_end: Option<usize>,
    lane: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
struct Edge {
    from: String,
    to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    back_edge: bool,
}

#[derive(Debug, Serialize)]
struct Payload {
    source: String,
    target: String,
    node_count: usize,
    edge_count: usize,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    start: String,
    end: String,
}

#[derive(Debug)]
struct GraphBuilder {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    next_id: usize,
}

impl GraphBuilder {
    fn new() -> Self {
        Self {
            nodes: vec![],
            edges: vec![],
            next_id: 1,
        }
    }

    fn add_node(
        &mut self,
        shape: &str,
        text: String,
        line_start: Option<usize>,
        line_end: Option<usize>,
        lane: i32,
    ) -> String {
        let id = format!("n{}", self.next_id);
        self.next_id += 1;
        self.nodes.push(Node {
            id: id.clone(),
            shape: shape.to_string(),
            text: text.chars().take(180).collect(),
            line_start,
            line_end: line_end.or(line_start),
            lane,
            level: None,
        });
        id
    }

    fn connect(&mut self, incoming: &[Exit], dst: &str, default_label: Option<&str>) {
        for ex in incoming {
            self.edges.push(Edge {
                from: ex.node_id.clone(),
                to: dst.to_string(),
                label: ex
                    .label
                    .clone()
                    .or_else(|| default_label.map(|s| s.to_string())),
                back_edge: false,
            });
        }
    }
}

fn strip_inline_comment(s: &str) -> String {
    // Minimal comment stripping; good enough for control-flow extraction.
    let mut out = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\'' && !in_double {
            in_single = !in_single;
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == '#' && !in_single && !in_double {
            break;
        }
        out.push(ch);
        i += 1;
    }
    out.trim_end().to_string()
}

fn leading_indent(raw: &str) -> usize {
    let mut indent = 0usize;
    for ch in raw.chars() {
        match ch {
            ' ' => indent += 1,
            '\t' => indent += 4,
            _ => break,
        }
    }
    indent
}

fn paren_delta(s: &str) -> i32 {
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\\' {
            i += 2;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            i += 1;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            i += 1;
            continue;
        }
        if !in_single && !in_double {
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
        }
        i += 1;
    }
    depth
}

fn build_logical_lines(physical: &[(usize, String)]) -> Vec<LineInfo> {
    let mut out: Vec<LineInfo> = vec![];
    let mut buf = String::new();
    let mut start_line = 0usize;
    let mut end_line = 0usize;
    let mut start_indent = 0usize;
    let mut depth = 0i32;

    for (lineno, raw) in physical {
        let no_comment = strip_inline_comment(raw);
        if no_comment.trim().is_empty() {
            continue;
        }
        if buf.is_empty() {
            start_line = *lineno;
            start_indent = leading_indent(&no_comment);
        }
        let part = no_comment.trim();
        if !buf.is_empty() {
            buf.push(' ');
        }
        buf.push_str(part);
        end_line = *lineno;
        depth += paren_delta(part);
        let continued = part.ends_with('\\') || depth > 0;
        if !continued {
            out.push(LineInfo {
                lineno: start_line,
                line_end: end_line,
                indent: start_indent,
                code: buf.trim().to_string(),
            });
            buf.clear();
            depth = 0;
        }
    }
    if !buf.is_empty() {
        out.push(LineInfo {
            lineno: start_line,
            line_end: end_line.max(start_line),
            indent: start_indent,
            code: buf.trim().to_string(),
        });
    }
    out
}

fn starts_with_kw(code: &str, kw: &str) -> bool {
    if !code.starts_with(kw) {
        return false;
    }
    code.len() == kw.len() || code.as_bytes()[kw.len()] == b' ' || code.as_bytes()[kw.len()] == b':'
}

fn is_groupable_process(code: &str) -> bool {
    !(starts_with_kw(code, "if")
        || starts_with_kw(code, "elif")
        || code == "else:"
        || starts_with_kw(code, "for")
        || starts_with_kw(code, "while")
        || starts_with_kw(code, "try")
        || starts_with_kw(code, "except")
        || starts_with_kw(code, "return")
        || starts_with_kw(code, "raise")
        || code == "break"
        || code == "continue"
        || starts_with_kw(code, "finally")
        || starts_with_kw(code, "with")
        || starts_with_kw(code, "def")
        || starts_with_kw(code, "class"))
}

fn stmt_text_from_line(code: &str) -> String {
    if starts_with_kw(code, "return") {
        return "Return".to_string();
    }
    if starts_with_kw(code, "raise") {
        return "Raise".to_string();
    }
    if starts_with_kw(code, "import") {
        let names = code.strip_prefix("import").unwrap_or("").trim();
        return format!("Import: {}", names);
    }
    if starts_with_kw(code, "from") && code.contains(" import ") {
        return format!("From {}", code.strip_prefix("from ").unwrap_or(code));
    }
    if starts_with_kw(code, "def") {
        return code.trim_end_matches(':').to_string();
    }
    if starts_with_kw(code, "class") {
        return code.trim_end_matches(':').to_string();
    }
    if starts_with_kw(code, "with") {
        return "with ...".to_string();
    }
    if code == "break" {
        return "break".to_string();
    }
    if code == "continue" {
        return "continue".to_string();
    }
    if code.contains("+=")
        || code.contains("-=")
        || code.contains("*=")
        || code.contains("/=")
        || code.contains("%=")
    {
        let lhs = code.split('=').next().unwrap_or("").trim();
        return format!("Update: {}", lhs);
    }
    if has_top_level_assignment(code) {
        let lhs = code.split('=').next().unwrap_or("").trim();
        let cleaned: Vec<String> = split_top_level_commas(lhs)
            .into_iter()
            .map(|s| s.trim().split(':').next().unwrap_or("").trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        return format!("Assign: {}", cleaned.join(", "));
    }
    format!(
        "Expr: {}",
        normalize_expr_text(code)
            .chars()
            .take(100)
            .collect::<String>()
    )
}

fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\\' {
            cur.push(ch);
            if i + 1 < chars.len() {
                i += 1;
                cur.push(chars[i]);
            }
            i += 1;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            cur.push(ch);
            i += 1;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            cur.push(ch);
            i += 1;
            continue;
        }
        if !in_single && !in_double {
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => {
                    out.push(cur.trim().to_string());
                    cur.clear();
                    i += 1;
                    continue;
                }
                _ => {}
            }
        }
        cur.push(ch);
        i += 1;
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    if out.is_empty() {
        out.push(s.trim().to_string());
    }
    out
}

fn has_top_level_assignment(code: &str) -> bool {
    let chars: Vec<char> = code.chars().collect();
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\\' && (in_single || in_double) {
            i += 2;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            i += 1;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            i += 1;
            continue;
        }
        if in_single || in_double {
            i += 1;
            continue;
        }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '=' if depth == 0 => {
                let prev = if i > 0 { Some(chars[i - 1]) } else { None };
                let next = if i + 1 < chars.len() {
                    Some(chars[i + 1])
                } else {
                    None
                };
                if prev == Some(':')
                    || prev == Some('=')
                    || prev == Some('!')
                    || prev == Some('<')
                    || prev == Some('>')
                    || next == Some('=')
                {
                    i += 1;
                    continue;
                }
                return true;
            }
            _ => {}
        }
        i += 1;
    }
    false
}

fn normalize_loop_header(code: &str) -> String {
    let trimmed = code.trim_end_matches(':').trim();
    if !starts_with_kw(trimmed, "for") {
        return trimmed.to_string();
    }
    let rest = trimmed.strip_prefix("for").unwrap_or(trimmed).trim();
    if let Some((target_raw, iter_raw)) = rest.split_once(" in ") {
        let target = target_raw.trim();
        let iter_expr = iter_raw.trim();
        let target_fmt =
            if target.contains(',') && !(target.starts_with('(') && target.ends_with(')')) {
                format!("({})", target)
            } else {
                target.to_string()
            };
        return format!("for {} in {}", target_fmt, iter_expr);
    }
    trimmed.to_string()
}

fn normalize_expr_text(code: &str) -> String {
    let s = code.trim();
    let chars: Vec<char> = s.chars().collect();
    let mut compact = String::with_capacity(s.len());
    for i in 0..chars.len() {
        let ch = chars[i];
        if ch == ' ' {
            let prev = if i > 0 { Some(chars[i - 1]) } else { None };
            let next = if i + 1 < chars.len() {
                Some(chars[i + 1])
            } else {
                None
            };
            if matches!(prev, Some('(' | '[' | '{')) || matches!(next, Some(')' | ']' | '}' | ','))
            {
                continue;
            }
        }
        compact.push(ch);
    }
    while compact.contains(",)") {
        compact = compact.replace(",)", ")");
    }
    while compact.contains(",]") {
        compact = compact.replace(",]", "]");
    }
    normalize_double_to_single_quotes(&compact)
}

fn normalize_double_to_single_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    let mut in_single = false;
    while i < chars.len() {
        let ch = chars[i];
        if in_single {
            out.push(ch);
            if ch == '\\' && i + 1 < chars.len() {
                i += 1;
                out.push(chars[i]);
            } else if ch == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if ch == '\'' {
            in_single = true;
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == '"' {
            // Collect double-quoted literal and convert to single quotes when safe.
            let mut j = i + 1;
            let mut segment = String::new();
            let mut escaped = false;
            let mut has_single = false;
            while j < chars.len() {
                let cj = chars[j];
                if escaped {
                    segment.push(cj);
                    escaped = false;
                    j += 1;
                    continue;
                }
                if cj == '\\' {
                    segment.push(cj);
                    escaped = true;
                    j += 1;
                    continue;
                }
                if cj == '\'' {
                    has_single = true;
                    segment.push(cj);
                    j += 1;
                    continue;
                }
                if cj == '"' {
                    break;
                }
                segment.push(cj);
                j += 1;
            }
            if j < chars.len() {
                if has_single {
                    out.push('"');
                    out.push_str(&segment);
                    out.push('"');
                } else {
                    out.push('\'');
                    out.push_str(&segment);
                    out.push('\'');
                }
                i = j + 1;
                continue;
            }
        }
        out.push(ch);
        i += 1;
    }
    out
}

fn summarize_group(lines: &[LineInfo]) -> String {
    let labels: Vec<String> = lines.iter().map(|l| stmt_text_from_line(&l.code)).collect();
    if labels.len() == 1 {
        return labels[0].clone();
    }
    let mut preview: Vec<String> = labels.iter().take(4).cloned().collect();
    if labels.len() > 4 {
        preview.push(format!("... +{} more", labels.len() - 4));
    }
    preview.join(" | ")
}

fn find_next_code(lines: &[LineInfo], idx: usize) -> Option<usize> {
    if idx >= lines.len() {
        None
    } else {
        Some(idx)
    }
}

fn body_start_indent(
    lines: &[LineInfo],
    idx: usize,
    current_indent: usize,
) -> Option<(usize, usize)> {
    if idx + 1 >= lines.len() {
        return None;
    }
    let ni = idx + 1;
    if lines[ni].indent > current_indent {
        Some((ni, lines[ni].indent))
    } else {
        None
    }
}

fn parse_if_chain(
    gb: &mut GraphBuilder,
    lines: &[LineInfo],
    mut idx: usize,
    indent: usize,
    incoming: Vec<Exit>,
    lane: i32,
    compact: bool,
    compact_max_stmts: usize,
) -> (Vec<Exit>, usize) {
    // Parse: if/elif*/else?
    let mut cur_in = incoming;
    let mut cur_lane = lane;
    let mut final_out: Vec<Exit> = vec![];
    loop {
        if idx >= lines.len() || lines[idx].indent != indent {
            final_out.extend(cur_in);
            return (final_out, idx);
        }
        let li = &lines[idx];
        let is_if = starts_with_kw(&li.code, "if");
        let is_elif = starts_with_kw(&li.code, "elif");
        if !is_if && !is_elif {
            break;
        }
        let cond = li
            .code
            .trim_end_matches(':')
            .split_once(' ')
            .map(|(_, rest)| rest.trim().to_string())
            .unwrap_or_else(|| li.code.clone());
        let did = gb.add_node(
            "decision",
            format!("if {}", cond),
            Some(li.lineno),
            Some(li.lineno),
            cur_lane,
        );
        gb.connect(&cur_in, &did, None);
        let yes_out = if let Some((bi, bindent)) = body_start_indent(lines, idx, indent) {
            let (o, ni) = walk_block(
                gb,
                lines,
                bi,
                bindent,
                vec![Exit {
                    node_id: did.clone(),
                    label: Some("yes".to_string()),
                }],
                cur_lane - 1,
                compact,
                compact_max_stmts,
            );
            idx = ni;
            o
        } else {
            idx += 1;
            vec![Exit {
                node_id: did.clone(),
                label: Some("yes".to_string()),
            }]
        };
        final_out.extend(yes_out);
        cur_in = vec![Exit {
            node_id: did,
            label: Some("no".to_string()),
        }];
        if idx >= lines.len()
            || lines[idx].indent != indent
            || (!starts_with_kw(&lines[idx].code, "elif") && lines[idx].code != "else:")
        {
            final_out.extend(cur_in);
            return (final_out, idx);
        }
        if lines[idx].code == "else:" {
            let else_out = if let Some((bi, bindent)) = body_start_indent(lines, idx, indent) {
                let (o, ni) = walk_block(
                    gb,
                    lines,
                    bi,
                    bindent,
                    cur_in.clone(),
                    cur_lane + 1,
                    compact,
                    compact_max_stmts,
                );
                idx = ni;
                o
            } else {
                idx += 1;
                cur_in.clone()
            };
            final_out.extend(else_out);
            return (final_out, idx);
        }
        // else this is an elif and loop continues as nested else->if (Python AST behavior).
        cur_lane += 1;
    }
    final_out.extend(cur_in);
    (final_out, idx)
}

#[allow(clippy::too_many_arguments)]
fn walk_block(
    gb: &mut GraphBuilder,
    lines: &[LineInfo],
    mut idx: usize,
    indent: usize,
    incoming: Vec<Exit>,
    lane: i32,
    compact: bool,
    compact_max_stmts: usize,
) -> (Vec<Exit>, usize) {
    let mut cur = incoming;
    while let Some(i) = find_next_code(lines, idx) {
        idx = i;
        if idx >= lines.len() {
            break;
        }
        let li = &lines[idx];
        if li.indent < indent {
            break;
        }
        if li.indent > indent {
            idx += 1;
            continue;
        }

        if starts_with_kw(&li.code, "if") {
            let (out, ni) = parse_if_chain(
                gb,
                lines,
                idx,
                indent,
                cur,
                lane,
                compact,
                compact_max_stmts,
            );
            cur = out;
            idx = ni;
            continue;
        }

        if starts_with_kw(&li.code, "for") || starts_with_kw(&li.code, "while") {
            let header = if starts_with_kw(&li.code, "for") {
                normalize_loop_header(&li.code)
            } else {
                li.code.trim_end_matches(':').to_string()
            };
            let lid = gb.add_node("loop", header, Some(li.lineno), Some(li.lineno), lane);
            gb.connect(&cur, &lid, None);
            idx += 1;
            let body_out = if idx < lines.len() && lines[idx].indent > indent {
                let bindent = lines[idx].indent;
                let (o, ni) = walk_block(
                    gb,
                    lines,
                    idx,
                    bindent,
                    vec![Exit {
                        node_id: lid.clone(),
                        label: Some("enter".to_string()),
                    }],
                    lane,
                    compact,
                    compact_max_stmts,
                );
                idx = ni;
                o
            } else {
                vec![Exit {
                    node_id: lid.clone(),
                    label: Some("enter".to_string()),
                }]
            };
            for ex in body_out {
                gb.edges.push(Edge {
                    from: ex.node_id,
                    to: lid.clone(),
                    label: Some("next".to_string()),
                    back_edge: false,
                });
            }
            if idx < lines.len() && lines[idx].indent == indent && lines[idx].code == "else:" {
                idx += 1;
                let orelse_out = if idx < lines.len() && lines[idx].indent > indent {
                    let bindent = lines[idx].indent;
                    let (o, ni) = walk_block(
                        gb,
                        lines,
                        idx,
                        bindent,
                        vec![Exit {
                            node_id: lid.clone(),
                            label: Some("done".to_string()),
                        }],
                        lane + 1,
                        compact,
                        compact_max_stmts,
                    );
                    idx = ni;
                    o
                } else {
                    vec![Exit {
                        node_id: lid.clone(),
                        label: Some("done".to_string()),
                    }]
                };
                cur = orelse_out;
            } else {
                cur = vec![Exit {
                    node_id: lid,
                    label: Some("done".to_string()),
                }];
            }
            continue;
        }

        if starts_with_kw(&li.code, "try") {
            let did = gb.add_node(
                "decision",
                "try/except".to_string(),
                Some(li.lineno),
                Some(li.lineno),
                lane,
            );
            gb.connect(&cur, &did, None);
            idx += 1;
            let mut out: Vec<Exit> = if idx < lines.len() && lines[idx].indent > indent {
                let bindent = lines[idx].indent;
                let (o, ni) = walk_block(
                    gb,
                    lines,
                    idx,
                    bindent,
                    vec![Exit {
                        node_id: did.clone(),
                        label: Some("try".to_string()),
                    }],
                    lane,
                    compact,
                    compact_max_stmts,
                );
                idx = ni;
                o
            } else {
                vec![Exit {
                    node_id: did.clone(),
                    label: Some("try".to_string()),
                }]
            };

            while idx < lines.len() && lines[idx].indent == indent {
                let c = lines[idx].code.trim_end_matches(':').to_string();
                if starts_with_kw(&c, "except") {
                    let lbl = c;
                    idx += 1;
                    if idx < lines.len() && lines[idx].indent > indent {
                        let bindent = lines[idx].indent;
                        let (o, ni) = walk_block(
                            gb,
                            lines,
                            idx,
                            bindent,
                            vec![Exit {
                                node_id: did.clone(),
                                label: Some(lbl),
                            }],
                            lane + 1,
                            compact,
                            compact_max_stmts,
                        );
                        idx = ni;
                        out.extend(o);
                    } else {
                        out.push(Exit {
                            node_id: did.clone(),
                            label: Some(lbl),
                        });
                    }
                } else if c == "else" {
                    idx += 1;
                    if idx < lines.len() && lines[idx].indent > indent {
                        let bindent = lines[idx].indent;
                        let (o, ni) = walk_block(
                            gb,
                            lines,
                            idx,
                            bindent,
                            vec![Exit {
                                node_id: did.clone(),
                                label: Some("else".to_string()),
                            }],
                            lane,
                            compact,
                            compact_max_stmts,
                        );
                        idx = ni;
                        out.extend(o);
                    }
                } else if c == "finally" {
                    idx += 1;
                    if idx < lines.len() && lines[idx].indent > indent {
                        let bindent = lines[idx].indent;
                        let (o, ni) = walk_block(
                            gb,
                            lines,
                            idx,
                            bindent,
                            out,
                            lane,
                            compact,
                            compact_max_stmts,
                        );
                        idx = ni;
                        out = o;
                    }
                } else {
                    break;
                }
            }
            if out.is_empty() {
                cur = vec![Exit {
                    node_id: did,
                    label: None,
                }];
            } else {
                cur = out;
            }
            continue;
        }

        if starts_with_kw(&li.code, "def") {
            let fn_id = gb.add_node(
                "process",
                stmt_text_from_line(&li.code),
                Some(li.lineno),
                Some(li.lineno),
                lane,
            );
            gb.connect(&cur, &fn_id, None);
            idx += 1;
            if idx < lines.len() && lines[idx].indent > indent {
                let bindent = lines[idx].indent;
                let (inner, ni) = walk_block(
                    gb,
                    lines,
                    idx,
                    bindent,
                    vec![Exit {
                        node_id: fn_id,
                        label: Some("enter".to_string()),
                    }],
                    lane + 1,
                    compact,
                    compact_max_stmts,
                );
                idx = ni;
                cur = inner;
            } else {
                cur = vec![Exit {
                    node_id: fn_id,
                    label: None,
                }];
            }
            continue;
        }

        if starts_with_kw(&li.code, "with") {
            let wn = gb.add_node(
                "process",
                "with ...".to_string(),
                Some(li.lineno),
                Some(li.lineno),
                lane,
            );
            gb.connect(&cur, &wn, None);
            idx += 1;
            if idx < lines.len() && lines[idx].indent > indent {
                let bindent = lines[idx].indent;
                let (o, ni) = walk_block(
                    gb,
                    lines,
                    idx,
                    bindent,
                    vec![Exit {
                        node_id: wn.clone(),
                        label: None,
                    }],
                    lane,
                    compact,
                    compact_max_stmts,
                );
                idx = ni;
                cur = o;
            } else {
                cur = vec![Exit {
                    node_id: wn,
                    label: None,
                }];
            }
            continue;
        }

        if compact && is_groupable_process(&li.code) {
            let mut group = vec![li.clone()];
            let mut j = idx + 1;
            while j < lines.len()
                && group.len() < compact_max_stmts
                && lines[j].indent == indent
                && is_groupable_process(&lines[j].code)
            {
                group.push(lines[j].clone());
                j += 1;
            }
            let text = summarize_group(&group);
            let pn = gb.add_node(
                "process",
                text,
                Some(group[0].lineno),
                Some(group[group.len() - 1].line_end),
                lane,
            );
            gb.connect(&cur, &pn, None);
            cur = vec![Exit {
                node_id: pn,
                label: None,
            }];
            idx = j;
            continue;
        }

        let pn = gb.add_node(
            "process",
            stmt_text_from_line(&li.code),
            Some(li.lineno),
            Some(li.lineno),
            lane,
        );
        gb.connect(&cur, &pn, None);
        cur = vec![Exit {
            node_id: pn,
            label: None,
        }];
        idx += 1;
    }
    (cur, idx)
}

fn assign_levels(nodes: &mut [Node], edges: &mut [Edge], start_id: &str, end_id: &str) {
    let mut line_vals: Vec<usize> = nodes
        .iter()
        .filter_map(|n| n.line_start)
        .filter(|v| *v > 0)
        .collect();
    line_vals.sort_unstable();
    line_vals.dedup();
    let mut rank = std::collections::HashMap::<usize, i32>::new();
    for (i, ln) in line_vals.iter().enumerate() {
        rank.insert(*ln, i as i32 + 1);
    }

    let mut level = std::collections::HashMap::<String, i32>::new();
    for n in nodes.iter() {
        if n.id == start_id {
            level.insert(n.id.clone(), 0);
        } else if let Some(ls) = n.line_start {
            level.insert(n.id.clone(), *rank.get(&ls).unwrap_or(&1));
        } else {
            let mx = rank.values().copied().max().unwrap_or(1);
            level.insert(n.id.clone(), mx + 1);
        }
    }

    let ecount = edges.len();
    for _ in 0..(nodes.len() * 4) {
        let mut changed = false;
        for e in edges.iter() {
            if e.label.as_deref() == Some("next") {
                continue;
            }
            let a = *level.get(&e.from).unwrap_or(&0);
            let b = *level.get(&e.to).unwrap_or(&0);
            if b <= a {
                level.insert(e.to.clone(), a + 1);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for n in nodes.iter_mut() {
        n.level = Some(*level.get(&n.id).unwrap_or(&0));
    }

    let mx = nodes.iter().filter_map(|n| n.level).max().unwrap_or(0);
    for n in nodes.iter_mut() {
        if n.id == end_id {
            n.level = Some(mx + 1);
        }
    }

    let lv: std::collections::HashMap<String, i32> = nodes
        .iter()
        .map(|n| (n.id.clone(), n.level.unwrap_or(0)))
        .collect();
    for e in edges.iter_mut().take(ecount) {
        let a = *lv.get(&e.from).unwrap_or(&0);
        let b = *lv.get(&e.to).unwrap_or(&0);
        e.back_edge = e.label.as_deref() == Some("next") || b <= a;
    }
}

fn extract_target_lines(src: &str, function: Option<&str>) -> Result<Vec<LineInfo>> {
    let raw_lines: Vec<String> = src.lines().map(|s| s.to_string()).collect();
    let physical_all: Vec<(usize, String)> = raw_lines
        .iter()
        .enumerate()
        .map(|(i, s)| (i + 1, s.clone()))
        .collect();
    if function.is_none() {
        return Ok(build_logical_lines(&physical_all));
    }
    let fn_name = function.unwrap();
    let mut def_line: Option<usize> = None;
    let mut def_indent = 0usize;
    for (idx, raw) in raw_lines.iter().enumerate() {
        let code = strip_inline_comment(raw).trim().to_string();
        if code.is_empty() {
            continue;
        }
        if (starts_with_kw(&code, "def") || starts_with_kw(&code, "async def"))
            && code.contains(&format!("{fn_name}("))
        {
            def_line = Some(idx + 1);
            def_indent = leading_indent(raw);
            break;
        }
    }
    let Some(def_lineno) = def_line else {
        bail!("Function '{}' not found", fn_name);
    };
    let mut physical: Vec<(usize, String)> = vec![];
    for (idx, raw) in raw_lines.iter().enumerate().skip(def_lineno) {
        let lineno = idx + 1;
        let code = strip_inline_comment(raw);
        if code.trim().is_empty() {
            continue;
        }
        let ind = leading_indent(raw);
        if ind <= def_indent {
            break;
        }
        physical.push((lineno, raw.clone()));
    }
    Ok(build_logical_lines(&physical))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let inp = cli.input_file.clone();
    let out = cli
        .output
        .clone()
        .unwrap_or_else(|| inp.with_extension("pyflow.json"));
    let src =
        fs::read_to_string(&inp).with_context(|| format!("failed to read {}", inp.display()))?;

    let fn_name = cli.function.trim().to_string();
    let target = if fn_name.is_empty() {
        "<module>".to_string()
    } else {
        fn_name.clone()
    };
    let lines = extract_target_lines(
        &src,
        if fn_name.is_empty() {
            None
        } else {
            Some(fn_name.as_str())
        },
    )?;

    let mut gb = GraphBuilder::new();
    let start = gb.add_node("terminator", "Start".to_string(), Some(1), Some(1), 0);
    let (exits, _) = walk_block(
        &mut gb,
        &lines,
        0,
        lines.first().map(|l| l.indent).unwrap_or(0),
        vec![Exit {
            node_id: start.clone(),
            label: None,
        }],
        0,
        !cli.no_compact,
        max(1, cli.compact_max_stmts),
    );
    let end = gb.add_node("terminator", "End".to_string(), None, None, 0);
    gb.connect(&exits, &end, None);

    assign_levels(&mut gb.nodes, &mut gb.edges, &start, &end);

    let payload = Payload {
        source: inp.to_string_lossy().to_string(),
        target,
        node_count: gb.nodes.len(),
        edge_count: gb.edges.len(),
        nodes: gb.nodes,
        edges: gb.edges,
        start,
        end,
    };
    fs::write(&out, serde_json::to_string_pretty(&payload)?)
        .with_context(|| format!("failed to write {}", out.display()))?;
    println!("Wrote {}", out.display());
    println!(
        "Nodes: {}, edges: {}",
        payload.node_count, payload.edge_count
    );
    Ok(())
}
