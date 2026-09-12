use anyhow::{Result, anyhow, ensure};
use lightningcss::stylesheet::{MinifyOptions, ParserOptions, PrinterOptions, StyleSheet};
use oxc::{
    allocator::Allocator,
    codegen::{Codegen, CodegenOptions},
    mangler::MangleOptions,
    minifier::{Minifier, MinifierOptions},
    parser::Parser,
    span::SourceType,
};

pub fn css(source: &str) -> Result<String> {
    let mut stylesheet = StyleSheet::parse(source, ParserOptions::default())
        .map_err(|error| anyhow!("parse bundled CSS: {error}"))?;
    stylesheet
        .minify(MinifyOptions::default())
        .map_err(|error| anyhow!("minify bundled CSS: {error}"))?;
    Ok(stylesheet
        .to_css(PrinterOptions {
            minify: true,
            ..Default::default()
        })
        .map_err(|error| anyhow!("print bundled CSS: {error}"))?
        .code)
}

pub fn javascript(source: &str) -> Result<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::cjs()).parse();
    ensure!(
        parsed.diagnostics.is_empty(),
        "parse bundled JavaScript: {:?}",
        parsed.diagnostics
    );
    let mut program = parsed.program;
    let result = Minifier::new(MinifierOptions {
        mangle: Some(MangleOptions {
            top_level: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    })
    .minify(&allocator, &mut program);
    Ok(Codegen::new()
        .with_options(CodegenOptions::minify())
        .with_scoping(result.scoping)
        .build(&program)
        .code)
}

/// Collapse layout whitespace while leaving preformatted and executable content byte-for-byte.
pub fn html(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    let mut pending_space = false;
    while !rest.is_empty() {
        if let Some(open) = ["<pre", "<code", "<script", "<style"]
            .iter()
            .filter_map(|tag| rest.find(tag).map(|index| (index, *tag)))
            .min_by_key(|(index, _)| *index)
        {
            let (index, tag) = open;
            let prefix = collapse_html_whitespace(&rest[..index], pending_space);
            out.push_str(&prefix);
            let Some(end) = rest[index..].find('>') else {
                out.push_str(&rest[index..]);
                break;
            };
            out.push_str(&rest[index..index + end + 1]);
            let close = format!("</{}>", &tag[1..]);
            let body_start = index + end + 1;
            if let Some(body_end) = rest[body_start..].find(&close) {
                out.push_str(&rest[body_start..body_start + body_end + close.len()]);
                rest = &rest[body_start + body_end + close.len()..];
                pending_space = false;
                continue;
            }
            out.push_str(&rest[body_start..]);
            break;
        }
        let collapsed = collapse_html_whitespace(rest, pending_space);
        out.push_str(&collapsed);
        break;
    }
    out
}

fn collapse_html_whitespace(source: &str, mut pending: bool) -> String {
    let mut out = String::new();
    for ch in source.chars() {
        if ch.is_whitespace() {
            pending = true;
        } else {
            if pending && !out.ends_with('>') && !out.is_empty() {
                out.push(' ');
            }
            pending = false;
            out.push(ch);
        }
    }
    out
}
