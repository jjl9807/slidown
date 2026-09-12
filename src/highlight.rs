use comrak::{adapters::SyntaxHighlighterAdapter, html};
use std::{
    borrow::Cow,
    collections::HashMap,
    fmt::{self, Write},
};
use syntect::{
    html::{ClassStyle, ClassedHTMLGenerator},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

pub struct Highlighter {
    syntaxes: SyntaxSet,
}

impl Highlighter {
    pub fn new() -> Self {
        Self {
            syntaxes: SyntaxSet::load_defaults_newlines(),
        }
    }
}

impl SyntaxHighlighterAdapter for Highlighter {
    fn write_highlighted(
        &self,
        output: &mut dyn Write,
        language: Option<&str>,
        code: &str,
    ) -> fmt::Result {
        let language = language.and_then(|s| s.split(',').next()).unwrap_or("");
        let syntax = self
            .syntaxes
            .find_syntax_by_token(language)
            .or_else(|| self.syntaxes.find_syntax_by_first_line(code))
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        let mut generator = ClassedHTMLGenerator::new_with_class_style(
            syntax,
            &self.syntaxes,
            ClassStyle::SpacedPrefixed { prefix: "syntax-" },
        );
        for line in LinesWithEndings::from(code) {
            if generator
                .parse_html_for_line_which_includes_newline(line)
                .is_err()
            {
                // Some grammars can exceed their regex backtracking limit. Never emit raw
                // source in the fallback: HTML-looking code must remain literal text.
                return html::escape(output, code);
            }
        }
        output.write_str(&generator.finalize())
    }

    fn write_pre_tag(
        &self,
        output: &mut dyn Write,
        _: HashMap<&'static str, Cow<'_, str>>,
    ) -> fmt::Result {
        html::write_opening_tag(output, "pre", [("class", "syntax-highlighting")])
    }

    fn write_code_tag(
        &self,
        output: &mut dyn Write,
        attributes: HashMap<&'static str, Cow<'_, str>>,
    ) -> fmt::Result {
        html::write_opening_tag(output, "code", attributes)
    }
}
