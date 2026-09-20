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

/// Collapse HTML whitespace runs to one space, including at inline element boundaries,
/// while leaving preformatted and executable content byte-for-byte.
pub fn html(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while !rest.is_empty() {
        if let Some(open) = ["<pre", "<code", "<script", "<style"]
            .iter()
            .filter_map(|tag| rest.find(tag).map(|index| (index, *tag)))
            .min_by_key(|(index, _)| *index)
        {
            let (index, tag) = open;
            let prefix = collapse_html_whitespace(&rest[..index]);
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
                continue;
            }
            out.push_str(&rest[body_start..]);
            break;
        }
        let collapsed = collapse_html_whitespace(rest);
        out.push_str(&collapsed);
        break;
    }
    out
}

fn collapse_html_whitespace(source: &str) -> String {
    let mut out = String::new();
    for ch in source.chars() {
        // Emit the space immediately so leading/trailing separators survive when
        // html() splits the input around a preserved element such as <code>.
        // Unicode spaces (e.g. NBSP and ideographic space) are content, not layout.
        if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{000c}') {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::html;

    #[test]
    fn html_preserves_inline_separators_without_adding_spaces() {
        for source in [
            "<p>中文 <strong>bold</strong> 中文 <mark>mark</mark> 中文</p>",
            "<p>中文 <code>code</code> 中文</p>",
            "<p><strong>bold</strong> <mark>mark</mark> <code>one</code> <code>two</code></p>",
            "<p>中文<strong>bold</strong><mark>mark</mark><code>code</code>中文</p>",
            "<p><strong> leading and trailing </strong>text</p>",
        ] {
            assert_eq!(html(source), source);
        }
    }

    #[test]
    fn html_collapses_only_html_whitespace() {
        assert_eq!(
            html("<p>中文  \t\r\n<strong>bold</strong>\n\t<code>code</code>\x0c 中文</p>"),
            "<p>中文 <strong>bold</strong> <code>code</code> 中文</p>"
        );
        let source = "<p>中文\u{a0}\u{a0}<mark>mark</mark>\u{3000}中文</p>";
        assert_eq!(html(source), source);
    }

    #[test]
    fn html_preserves_preformatted_and_executable_contents() {
        let source = "<pre><code>  one\n\t two  </code>\n</pre> \
                      <code>one  two</code> \
                      <script>const text = 'one  two';\n</script> \
                      <style>/* keep  spacing */\np { color: red; }</style>";
        assert_eq!(html(source), source);
    }
}
