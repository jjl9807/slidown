use crate::highlight::Highlighter;
use anyhow::{Context, Result, bail, ensure};
use comrak::html::{self, ChildRendering};
use comrak::{
    Arena, Options,
    nodes::{AlertType, AstNode, NodeValue},
    options::Plugins,
};
use percent_encoding::percent_decode_str;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::{self, Write},
    fs,
    path::{Path, PathBuf},
};
use syntect::{
    highlighting::ThemeSet,
    html::{ClassStyle, css_for_theme_with_class_style},
};

pub struct Deck {
    pub files: BTreeMap<String, Vec<u8>>,
    pub warnings: Vec<String>,
    pub slides: usize,
}

pub struct Compiler {
    pub dependencies: BTreeSet<PathBuf>,
    highlighter: Highlighter,
    highlight_css: String,
    cache: HashMap<String, String>,
}

#[derive(Default)]
struct Metadata {
    title: Option<String>,
    author: Option<String>,
    date: Option<String>,
    email: Option<String>,
    closing: Option<String>,
}

#[derive(Default)]
struct RenderData {
    replacements: HashMap<usize, String>,
    headings: HashMap<usize, String>,
}

fn key(node: &AstNode<'_>) -> usize {
    std::ptr::from_ref(node) as usize
}

pub fn escape(s: &str) -> String {
    let mut output = String::new();
    html::escape(&mut output, s).expect("writing to String");
    output
}

fn plain<'a>(node: &'a AstNode<'a>) -> String {
    node.descendants()
        .filter_map(|n| match &n.data.borrow().value {
            NodeValue::Text(s) => Some(s.to_string()),
            NodeValue::Code(c) => Some(c.literal.clone()),
            NodeValue::Math(m) => Some(m.literal.clone()),
            NodeValue::SoftBreak | NodeValue::LineBreak => Some(" ".into()),
            _ => None,
        })
        .collect()
}

fn metadata(source: &str, path: &Path) -> Result<(Metadata, String, Vec<String>)> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let lines: Vec<_> = source.lines().collect();
    let mut meta = Metadata::default();
    let mut warnings = Vec::new();
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return Ok((meta, source.to_owned(), warnings));
    }
    let end = (1..lines.len())
        .find(|&i| lines[i].trim_end() == "---")
        .with_context(|| format!("{}:1: unclosed YAML front matter", path.display()))?;
    let yaml = lines[1..end].join("\n");
    let value: serde_yaml::Value = serde_yaml::from_str(&yaml).map_err(|e| {
        let line = e.location().map_or(2, |l| l.line() + 1);
        anyhow::anyhow!("{}:{line}: invalid YAML: {e}", path.display())
    })?;
    if !value.is_null() {
        let mapping = value
            .as_mapping()
            .with_context(|| format!("{}:2: front matter must be a mapping", path.display()))?;
        for (name, value) in mapping {
            let name = name.as_str().with_context(|| {
                format!("{}:2: front matter keys must be strings", path.display())
            })?;
            let line = lines[1..end]
                .iter()
                .position(|l| l.trim_start().starts_with(&format!("{name}:")))
                .map_or(2, |i| i + 2);
            let field = match name {
                "title" => &mut meta.title,
                "author" => &mut meta.author,
                "date" => &mut meta.date,
                "email" => &mut meta.email,
                "closing" => &mut meta.closing,
                _ => {
                    warnings.push(format!(
                        "{}:{line}: unknown front matter field `{name}`",
                        path.display()
                    ));
                    continue;
                }
            };
            if value.is_null() {
                continue;
            }
            let text = value
                .as_str()
                .with_context(|| format!("{}:{line}: `{name}` must be a string", path.display()))?;
            if !text.trim().is_empty() {
                *field = Some(text.to_owned());
            }
        }
    }
    // Blank the YAML region rather than removing it, preserving source line numbers.
    let body = "\n".repeat(end + 1) + &lines[end + 1..].join("\n");
    Ok((meta, body, warnings))
}

fn image_path(url: &str, parent: &Path) -> Result<Option<(PathBuf, String)>> {
    if url.starts_with("//") || url.starts_with('#') {
        return Ok(None);
    }
    if let Some((scheme, _)) = url.split_once(':')
        && !scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    {
        return Ok(None);
    }
    let end = url.find(['?', '#']).unwrap_or(url.len());
    let local = percent_decode_str(&url[..end])
        .decode_utf8()
        .context("image path is not valid UTF-8")?;
    ensure!(!local.is_empty(), "empty local image path");
    Ok(Some((parent.join(local.as_ref()), url[end..].to_owned())))
}

impl Compiler {
    pub fn new() -> Self {
        let highlighter = Highlighter::new();
        let themes = ThemeSet::load_defaults();
        let highlight_css = css_for_theme_with_class_style(
            &themes.themes["InspiredGitHub"],
            ClassStyle::SpacedPrefixed { prefix: "syntax-" },
        )
        .expect("built-in theme");
        Self {
            dependencies: BTreeSet::new(),
            highlighter,
            highlight_css,
            cache: HashMap::new(),
        }
    }

    pub fn compile(&mut self, outline: &Path) -> Result<Deck> {
        let path = std::path::absolute(outline)?;
        self.dependencies.clear();
        self.dependencies.insert(path.clone());
        let source =
            fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let (meta, body, warnings) = metadata(&source, &path)?;
        let arena = Arena::new();
        let mut options = Options::default();
        options.extension.table = true;
        options.extension.strikethrough = true;
        options.extension.highlight = true;
        options.extension.tasklist = true;
        options.render.tasklist_classes = true;
        options.extension.autolink = true;
        options.extension.alerts = true;
        options.extension.math_dollars = true;
        // Recognize footnotes solely to produce a useful unsupported-syntax error.
        options.extension.footnotes = true;
        options.extension.inline_footnotes = true;
        let root = comrak::parse_document(&arena, &body, &options);
        let nodes: Vec<_> = root.descendants().collect();
        let parent = path.parent().expect("absolute input path has a parent");
        // Discover all images before rendering: missing/new images remain watched after errors.
        for node in &nodes {
            if let NodeValue::Image(image) = &node.data.borrow().value
                && let Some((image_path, _)) =
                    image_path(&image.url, parent).with_context(|| {
                        format!(
                            "{}:{}",
                            path.display(),
                            node.data.borrow().sourcepos.start.line
                        )
                    })?
            {
                self.dependencies.insert(image_path);
            }
        }
        for node in &nodes {
            let data = node.data.borrow();
            let location = format!(
                "{}:{}:{}",
                path.display(),
                data.sourcepos.start.line,
                data.sourcepos.start.column
            );
            match &data.value {
                NodeValue::HtmlBlock(_) | NodeValue::HtmlInline(_) => {
                    bail!("{location}: raw HTML is not supported")
                }
                NodeValue::FootnoteDefinition(_) | NodeValue::FootnoteReference(_) => {
                    bail!("{location}: footnotes are not supported")
                }
                _ => (),
            }
        }
        let blocks: Vec<_> = root.children().collect();
        let h1s: Vec<_> = nodes
            .iter()
            .copied()
            .filter(|n| matches!(n.data.borrow().value, NodeValue::Heading(ref h) if h.level == 1))
            .collect();
        ensure!(
            h1s.len() == 1,
            "{}:1: expected exactly one H1, found {}",
            path.display(),
            h1s.len()
        );
        let cover = h1s[0];
        ensure!(
            blocks.first().is_some_and(|n| std::ptr::eq(*n, cover)),
            "{}:{}: H1 must be the first document block",
            path.display(),
            cover.data.borrow().sourcepos.start.line
        );
        let first_slide = blocks.iter().position(
            |n| matches!(n.data.borrow().value, NodeValue::Heading(ref h) if h.level == 2),
        );
        let title_end = cover.data.borrow().sourcepos.end.line;
        let body_lines: Vec<_> = body.lines().collect();
        if let Some((i, _)) = body_lines
            .iter()
            .enumerate()
            .take(cover.data.borrow().sourcepos.start.line.saturating_sub(1))
            .find(|(_, l)| !l.trim().is_empty())
        {
            bail!(
                "{}:{}: content before H1 is not allowed",
                path.display(),
                i + 1
            );
        }
        let next_line = first_slide.map_or(body_lines.len() + 1, |i| {
            blocks[i].data.borrow().sourcepos.start.line
        });
        for (i, line) in body_lines
            .iter()
            .enumerate()
            .take(next_line - 1)
            .skip(title_end)
        {
            ensure!(
                line.trim().is_empty(),
                "{}:{}: only blank lines are allowed between H1 and the first H2",
                path.display(),
                i + 1
            );
        }
        let mut deck = Deck {
            files: BTreeMap::new(),
            warnings,
            slides: 1,
        };
        let mut data = RenderData::default();
        let mut ids = HashSet::new();
        for (index, node) in nodes.iter().enumerate() {
            let value = node.data.borrow().value.clone();
            let line = node.data.borrow().sourcepos.start.line;
            let result: Result<()> = (|| {
                match value {
                    NodeValue::Heading(_) => {
                        let base = plain(node)
                            .to_lowercase()
                            .chars()
                            .filter_map(|c| {
                                if c.is_whitespace() {
                                    Some('-')
                                } else if c.is_alphanumeric() || "-_".contains(c) {
                                    Some(c)
                                } else {
                                    None
                                }
                            })
                            .collect::<String>();
                        let base = if base.is_empty() {
                            "heading".to_owned()
                        } else {
                            base
                        };
                        let mut id = base.clone();
                        let mut suffix = 1;
                        while !ids.insert(id.clone()) {
                            id = format!("{base}-{suffix}");
                            suffix += 1;
                        }
                        data.headings.insert(key(node), id);
                    }
                    NodeValue::Image(mut image) => {
                        if let Some((source, suffix)) = image_path(&image.url, parent)? {
                            let bytes = fs::read(&source)
                                .with_context(|| format!("read image {}", source.display()))?;
                            let extension = source
                                .extension()
                                .and_then(|s| s.to_str())
                                .unwrap_or("bin")
                                .to_ascii_lowercase();
                            ensure!(
                                extension.chars().all(|c| c.is_ascii_alphanumeric()),
                                "invalid image extension"
                            );
                            let target = format!(
                                "assets/images/{}.{}",
                                crate::build::digest(&bytes),
                                extension
                            );
                            deck.files.insert(target.clone(), bytes);
                            image.url = target + &suffix;
                            node.data.borrow_mut().value = NodeValue::Image(image);
                        }
                    }
                    NodeValue::CodeBlock(code)
                        if code.info.split_whitespace().next() == Some("mermaid") =>
                    {
                        let cache_key = format!("mermaid:{}", code.literal);
                        let svg = if let Some(cached) = self.cache.get(&cache_key) {
                            cached.clone()
                        } else {
                            validate_mermaid_header(&code.literal)?;
                            let svg = mermaid_rs_renderer::render(&code.literal)
                                .context("Mermaid rendering failed")?;
                            self.cache.insert(cache_key, svg.clone());
                            svg
                        };
                        let svg = namespace_svg(&svg, &format!("slidown:diagram:{index}:"))?;
                        data.replacements.insert(key(node), format!("<div class=\"mermaid\" role=\"img\" aria-label=\"{}\">{svg}</div>\n", escape(&code.literal)));
                    }
                    NodeValue::Math(math) => {
                        let cache_key = format!("math:{}:{}", math.display_math, math.literal);
                        let svg = if let Some(cached) = self.cache.get(&cache_key) {
                            cached.clone()
                        } else {
                            let rendered = render_math(&math.literal, math.display_math)?;
                            self.cache.insert(cache_key, rendered.clone());
                            rendered
                        };
                        data.replacements.insert(
                            key(node),
                            namespace_svg(&svg, &format!("slidown:math:{index}:"))?,
                        );
                    }
                    _ => (),
                }
                Ok(())
            })();
            result.with_context(|| format!("{}:{line}", path.display()))?;
        }
        // Bound memory across long editing sessions.
        if self.cache.len() > 256 {
            self.cache.clear();
        }
        let mut plugins = Plugins::default();
        plugins.render.codefence_syntax_highlighter = Some(&self.highlighter);
        let render = |node| -> Result<String> {
            let mut output = String::new();
            html::format_document_with_formatter(
                node,
                &options,
                &mut output,
                &plugins,
                formatter,
                &data,
            )?;
            Ok(output)
        };
        let mut content = format!(
            "<section class=\"slide cover active no-anim\"><div class=\"titlebar\">{}</div><div class=\"inner\">",
            render(cover)?
        );
        if meta.author.is_some() || meta.date.is_some() || meta.email.is_some() {
            content.push_str("<p class=\"cover-meta\">");
            let mut parts = Vec::new();
            if let Some(author) = &meta.author {
                parts.push(escape(author));
            }
            if let Some(email) = &meta.email {
                let href = percent_encoding::utf8_percent_encode(
                    email,
                    percent_encoding::NON_ALPHANUMERIC,
                )
                .to_string();
                parts.push(format!("<a href=\"mailto:{href}\">{}</a>", escape(email)));
            }
            if let Some(date) = &meta.date {
                parts.push(escape(date));
            }
            content.push_str(&parts.join("<br>"));
            content.push_str("</p>");
        }
        content.push_str("</div></section>\n");
        if let Some(first) = first_slide {
            for (i, block) in blocks.iter().enumerate().skip(first) {
                if matches!(block.data.borrow().value, NodeValue::Heading(ref h) if h.level == 2) {
                    if i != first {
                        content.push_str("</div></section>\n");
                    }
                    write!(
                        content,
                        "<section class=\"slide\"><div class=\"titlebar\">{}</div><div class=\"inner\">",
                        render(block)?
                    )?;
                    deck.slides += 1;
                } else {
                    content.push_str(&render(block)?);
                }
            }
            content.push_str("</div></section>\n");
        }
        if let Some(closing) = meta.closing {
            write!(
                content,
                "<section class=\"slide cover closing\"><div class=\"titlebar\"><h2>{}</h2></div><div class=\"inner\"></div></section>",
                escape(&closing)
            )?;
            deck.slides += 1;
        }
        let title = escape(&meta.title.unwrap_or_else(|| plain(cover)));
        let html = format!(
            "<!doctype html>\n<!-- Generated by slidown -->\n<html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{title}</title><link rel=\"stylesheet\" href=\"assets/slides.css\"><link rel=\"stylesheet\" href=\"assets/highlight.css\"></head><body>{content}{}<script src=\"assets/slides.js\"></script></body></html>\n",
            include_str!("../assets/footer.html")
        );
        deck.files.insert("index.html".into(), html.into_bytes());
        deck.files.insert(
            "assets/slides.css".into(),
            include_bytes!("../assets/slides.css").to_vec(),
        );
        deck.files.insert(
            "assets/slides.js".into(),
            include_bytes!("../assets/slides.js").to_vec(),
        );
        deck.files.insert(
            "assets/highlight.css".into(),
            self.highlight_css.as_bytes().to_vec(),
        );
        Ok(deck)
    }
}

fn formatter<'a>(
    context: &mut html::Context<'_, '_, &RenderData>,
    node: &'a AstNode<'a>,
    entering: bool,
) -> Result<ChildRendering, fmt::Error> {
    if let Some(replacement) = context.user.replacements.get(&key(node)) {
        if entering {
            context.write_str(replacement)?;
        }
        return Ok(ChildRendering::Skip);
    }
    match &node.data.borrow().value {
        NodeValue::Heading(h) => {
            if entering {
                write!(
                    context,
                    "<h{} id=\"{}\">",
                    h.level,
                    escape(&context.user.headings[&key(node)])
                )?;
            } else {
                writeln!(context, "</h{}>", h.level)?;
            }
            Ok(ChildRendering::HTML)
        }
        NodeValue::Alert(alert) => {
            if entering {
                let (name, icon) = match alert.alert_type {
                    AlertType::Note => ("note", include_str!("../assets/alert-note.svg")),
                    AlertType::Tip => ("tip", include_str!("../assets/alert-tip.svg")),
                    AlertType::Important => {
                        ("important", include_str!("../assets/alert-important.svg"))
                    }
                    AlertType::Warning => ("warning", include_str!("../assets/alert-warning.svg")),
                    AlertType::Caution => ("caution", include_str!("../assets/alert-caution.svg")),
                };
                let icon =
                    icon.replacen("<svg ", "<svg class=\"octicon\" aria-hidden=\"true\" ", 1);
                write!(
                    context,
                    "<div class=\"markdown-alert markdown-alert-{name}\"><p class=\"markdown-alert-title\">{icon}{}</p>",
                    alert.alert_type.default_title()
                )?;
            } else {
                context.write_str("</div>\n")?;
            }
            Ok(ChildRendering::HTML)
        }
        _ => html::format_node_default(context, node, entering),
    }
}

fn render_math(latex: &str, display: bool) -> Result<String> {
    let nodes = ratex_parser::parse(latex).context("LaTeX parsing failed")?;
    let options = ratex_layout::LayoutOptions {
        style: if display {
            ratex_types::MathStyle::Display
        } else {
            ratex_types::MathStyle::Text
        },
        ..Default::default()
    };
    let list = ratex_layout::to_display_list(&ratex_layout::layout(&nodes, &options));
    let svg = ratex_svg::render_to_svg(
        &list,
        &ratex_svg::SvgOptions {
            embed_glyphs: true,
            padding: 0.0,
            ..Default::default()
        },
    );
    ensure!(
        !svg.contains("<text"),
        "LaTeX contains a glyph without an embedded font; install a suitable font or set RATEX_UNICODE_FONT"
    );
    let height = list.height + list.depth;
    let svg = svg.replacen(
        "<svg ",
        &format!(
            "<svg style=\"width:{:.6}em;height:{:.6}em;vertical-align:-{:.6}em\" ",
            list.width, height, list.depth
        ),
        1,
    );
    Ok(format!(
        "<span class=\"math {}\" role=\"img\" aria-label=\"{}\">{svg}</span>",
        if display {
            "math-display"
        } else {
            "math-inline"
        },
        escape(latex)
    ))
}

// Rewrite local SVG references as well as IDs, so multiple diagrams never share markers.
fn namespace_svg(svg: &str, prefix: &str) -> Result<String> {
    use quick_xml::{
        Reader, Writer,
        events::{BytesStart, Event},
    };
    let mut reader = Reader::from_str(svg);
    let mut writer = Writer::new(Vec::new());
    loop {
        let event = reader.read_event()?;
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let name = std::str::from_utf8(start.name().as_ref())?.to_owned();
                let mut output = BytesStart::new(name);
                for attribute in start.attributes() {
                    let attr = attribute?;
                    let attr_name = std::str::from_utf8(attr.key.as_ref())?;
                    let value = attr.decoded_and_normalized_value(
                        quick_xml::XmlVersion::Implicit1_0,
                        reader.decoder(),
                    )?;
                    let value = if attr_name == "id" {
                        format!("{prefix}{value}")
                    } else if (attr_name == "href" || attr_name == "xlink:href")
                        && value.starts_with('#')
                    {
                        format!("#{prefix}{}", &value[1..])
                    } else {
                        value.replace("url(#", &format!("url(#{prefix}"))
                    };
                    output.push_attribute((attr_name, value.as_str()));
                }
                if matches!(event, Event::Empty(_)) {
                    writer.write_event(Event::Empty(output))?;
                } else {
                    writer.write_event(Event::Start(output))?;
                }
            }
            Event::Decl(_) => (),
            Event::Eof => break,
            event => writer.write_event(event)?,
        }
    }
    Ok(String::from_utf8(writer.into_inner())?)
}

fn validate_mermaid_header(source: &str) -> Result<()> {
    let mut front_matter = false;
    for line in source.lines().map(str::trim) {
        if line == "---" {
            front_matter = !front_matter;
            continue;
        }
        if front_matter || line.is_empty() || line.starts_with("%%") {
            continue;
        }
        let header = line
            .split(|c: char| c.is_whitespace() || c == ';')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        ensure!(
            [
                "flowchart",
                "graph",
                "sequencediagram",
                "classdiagram",
                "statediagram",
                "statediagram-v2",
                "erdiagram",
                "pie",
                "mindmap",
                "journey",
                "timeline",
                "gantt",
                "requirementdiagram",
                "gitgraph",
                "c4context",
                "c4container",
                "c4component",
                "c4dynamic",
                "c4deployment",
                "sankey-beta",
                "sankey",
                "quadrantchart",
                "zenuml",
                "block-beta",
                "block",
                "packet-beta",
                "packet",
                "kanban",
                "architecture-beta",
                "architecture",
                "radar-beta",
                "radar",
                "treemap-beta",
                "treemap",
                "xychart-beta",
                "xychart",
            ]
            .contains(&header.as_str()),
            "unknown or missing Mermaid diagram header `{header}`"
        );
        return Ok(());
    }
    bail!("empty Mermaid diagram")
}
