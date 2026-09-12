use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

fn build(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_slidown"));
    command.current_dir(root).arg("build");
    command.args(args).output().unwrap()
}
fn success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn html(root: &Path) -> String {
    fs::read_to_string(root.join("index.html")).unwrap()
}
fn fixture(source: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("OUTLINE.md"), source).unwrap();
    dir
}

#[test]
fn metadata_are_independent_from_cover_title() {
    let dir = fixture(
        "---\ntitle: 'Browser <title>'\nauthor: 'A & B'\ndate: 2026-09-11\nemail: a@example.com\nclosing: 'Thank you <all>'\nextra: {anything: true}\n---\n# Cover **title**\n\n## First\nHello\n",
    );
    let result = build(dir.path(), &[]);
    assert!(String::from_utf8_lossy(&result.stderr).contains("warning:"));
    assert!(String::from_utf8_lossy(&result.stderr).contains("extra"));
    success(result);
    let page = html(&dir.path().join("dist"));
    assert!(page.contains("<title>Browser &lt;title&gt;</title>"));
    assert!(page.contains("Cover <strong>title</strong>"));
    assert!(page.contains(
        "A &amp; B<br><a href=\"mailto:a%40example%2Ecom\">a@example.com</a><br>2026-09-11"
    ));
    assert!(page.contains("mailto:a%40example%2Ecom"));
    assert!(page.contains("Thank you &lt;all&gt;"));
    assert_eq!(page.matches("<section class=\"slide").count(), 3);
    assert!(!page.contains("__slidown"));
    assert!(page.contains("<style>"));
    assert!(page.contains("<script>"));
    assert!(!page.contains("<script src="));
    assert!(!page.contains("<link rel=\"stylesheet\""));
    assert!(!dir.path().join("dist/assets").exists());
}

#[test]
fn cover_only_and_empty_metadata() {
    let dir = fixture("---\ntitle: ''\nclosing: null\nauthor: null\n---\n# Only `title`\n\n");
    success(build(dir.path(), &[]));
    let page = html(&dir.path().join("dist"));
    assert_eq!(page.matches("<section class=\"slide").count(), 1);
    assert!(page.contains("<title>Only title</title>"));
    assert!(!page.contains("class=\"cover-meta\""));
}

#[test]
fn invalid_structure_reports_location_without_writing_output() {
    let cases = [
        ("## Missing\n", "exactly one H1"),
        ("# First\n## Page\n# Again\n", "exactly one H1"),
        ("Text\n# Cover\n", "first document block"),
        ("# Cover\n\nUnexpected\n\n## Page", ":3: only blank lines"),
        (
            "# Cover\n[ref]: https://example.com\n## Page",
            ":2: only blank lines",
        ),
        (
            "[ref]: https://example.com\n# Cover\n## Page",
            ":1: content before H1",
        ),
        ("# Cover\n\n### Subheading", "only blank lines"),
        ("# Cover\n<!-- comment -->\n## Page", "raw HTML"),
        ("# Cover\n## Page\nA <b>bold</b> word", "raw HTML"),
        ("# Cover\n## Page\nFootnote[^1]\n\n[^1]: Text", "footnotes"),
        (
            "---\nauthor: [A, B]\n---\n# Cover",
            "`author` must be a string",
        ),
        ("---\ntitle: [\n---\n# Cover", "invalid YAML"),
        ("---\ntitle: Test\n# Cover", "unclosed YAML"),
    ];
    for (source, expected) in cases {
        let dir = fixture(source);
        let result = build(dir.path(), &[]);
        assert!(!result.status.success(), "accepted {source}");
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains(expected), "expected {expected}, got {error}");
        assert!(!dir.path().join("index.html").exists());
    }
}

#[test]
fn split_ast_preserves_nested_headings_references_and_code() {
    let dir = fixture(
        "# Cover\n## Page\n### Detail\n> ## Quoted\n\n```markdown\n# Literal\n## Literal\n```\n\n[reference][target]\n\n---\n\n## Page\n## Page-1\n\n[target]: https://example.com\n",
    );
    success(build(dir.path(), &[]));
    let page = html(&dir.path().join("dist"));
    assert_eq!(page.matches("<section class=\"slide").count(), 4);
    assert!(page.contains("<h3 id=\"detail\">Detail</h3>"));
    assert!(page.contains("<blockquote>"));
    assert!(page.contains("href=\"https://example.com\""));
    assert!(page.contains("id=\"page-1-1\""));
    assert!(page.contains("<hr />"));
}

#[test]
fn local_images_resolve_from_outline_and_remote_images_are_untouched() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("notes/a")).unwrap();
    fs::create_dir_all(dir.path().join("notes/b")).unwrap();
    fs::write(dir.path().join("notes/a/图 片.svg"), "image-one").unwrap();
    fs::write(dir.path().join("notes/b/图 片.svg"), "image-two").unwrap();
    fs::write(dir.path().join("notes/talk.md"), "# Cover\n## Images\n![one](a/图%20片.svg#view)\n![duplicate][same]\n![two](b/图%20片.svg)\n![web](https://example.invalid/image.png)\n![data](data:image/png;base64,AAAA)\n\n[same]: a/图%20片.svg \"image title\"\n").unwrap();
    success(build(
        dir.path(),
        &["--outline", "notes/talk.md", "--output", "site"],
    ));
    let page = html(&dir.path().join("site"));
    assert!(page.contains(".svg#view"));
    assert!(page.contains("title=\"image title\""));
    assert!(page.contains("https://example.invalid/image.png"));
    assert!(page.contains("data:image/png;base64,AAAA"));
    assert_eq!(
        fs::read_dir(dir.path().join("site/assets"))
            .unwrap()
            .count(),
        2
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("notes/a/图 片.svg")).unwrap(),
        "image-one"
    );
}

#[test]
fn failed_render_keeps_last_output_and_assets() {
    let dir = fixture("# Cover\n## Good\nOriginal");
    success(build(dir.path(), &[]));
    let before = html(&dir.path().join("dist"));
    for source in [
        "# Cover\n## Broken\n![missing](missing.png)",
        "# Cover\n## Broken\n```mermaid\nnot-a-diagram\n```",
        "# Cover\n## Broken\n$\\unknownSlidownCommand{x}$",
    ] {
        fs::write(dir.path().join("OUTLINE.md"), source).unwrap();
        let result = build(dir.path(), &[]);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("OUTLINE.md:"));
        assert_eq!(html(&dir.path().join("dist")), before);
    }
}

#[test]
fn publishing_replaces_the_entire_output_directory() {
    let dir = fixture("# Cover\n## Images\n![test](input.svg)");
    fs::write(dir.path().join("input.svg"), "image-data").unwrap();
    let output = dir.path().join("dist");
    fs::create_dir_all(output.join("assets")).unwrap();
    fs::write(output.join("assets/keep.txt"), "old file").unwrap();
    fs::write(
        output.join("assets/.slidown-manifest.json"),
        "obsolete manifest",
    )
    .unwrap();
    fs::write(output.join("index.html"), "existing website").unwrap();
    success(build(dir.path(), &[]));
    assert!(!output.join("assets/keep.txt").exists());
    assert!(!output.join("assets/.slidown-manifest.json").exists());
    assert_eq!(fs::read_dir(output.join("assets")).unwrap().count(), 1);
    fs::write(output.join("index.html"), "manually edited output").unwrap();
    fs::write(output.join("notes.txt"), "also remove this").unwrap();
    fs::write(dir.path().join("OUTLINE.md"), "# Cover\n## No images").unwrap();
    success(build(dir.path(), &[]));
    assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
    assert!(html(&output).contains("No images"));
    assert_eq!(
        fs::read_to_string(dir.path().join("input.svg")).unwrap(),
        "image-data"
    );
}

#[cfg(unix)]
#[test]
fn replacing_output_does_not_follow_nested_symlinks() {
    let dir = fixture("# Cover");
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join("keep.txt"), "outside output").unwrap();
    fs::create_dir(dir.path().join("dist")).unwrap();
    std::os::unix::fs::symlink(other.path(), dir.path().join("dist/assets")).unwrap();
    success(build(dir.path(), &[]));
    assert!(!dir.path().join("dist/assets").exists());
    assert_eq!(
        fs::read_to_string(other.path().join("keep.txt")).unwrap(),
        "outside output"
    );
}

#[test]
fn output_must_be_separate_from_working_directory_and_inputs() {
    let dir = fixture("# Cover");
    for output in [".", ".."] {
        let result = build(dir.path(), &["--output", output]);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("output"));
        assert!(dir.path().join("OUTLINE.md").exists());
    }
    fs::create_dir(dir.path().join("site")).unwrap();
    fs::write(dir.path().join("site/image.svg"), "source").unwrap();
    fs::write(
        dir.path().join("OUTLINE.md"),
        "# Cover\n## Image\n![source](site/image.svg)",
    )
    .unwrap();
    let result = build(dir.path(), &["--output", "site"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("must not contain input"));
    assert_eq!(
        fs::read_to_string(dir.path().join("site/image.svg")).unwrap(),
        "source"
    );
}

#[cfg(unix)]
#[test]
fn output_directory_cannot_be_a_symlink() {
    let dir = fixture("# Cover");
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join("keep.txt"), "outside output").unwrap();
    std::os::unix::fs::symlink(other.path(), dir.path().join("dist")).unwrap();
    let result = build(dir.path(), &[]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symlink"));
    assert!(other.path().join("keep.txt").exists());
}

#[test]
fn native_svgs_alerts_and_highlighting_render_offline() {
    let dir = tempfile::tempdir().unwrap();
    let source = format!("{}/examples/OUTLINE.md", env!("CARGO_MANIFEST_DIR"));
    success(build(dir.path(), &["--outline", &source]));
    let page = html(&dir.path().join("dist"));
    assert_eq!(page.matches("<section class=\"slide").count(), 13);
    assert!(page.contains("<mark>Keep this takeaway in mind</mark>"));
    assert!(page.contains("<del>A shorter way to strike text</del>"));
    assert!(page.contains("syntax-keyword"));
    assert!(page.contains("math-inline"));
    assert!(page.contains("math-display"));
    assert!(!page.contains("<script src=\"http"));
    for kind in ["note", "tip", "important", "warning", "caution"] {
        assert!(page.contains(&format!("markdown-alert-{kind}")));
    }
    let mut count = 0;
    for section in page.split("<svg").skip(1) {
        let end = section.find("</svg>").unwrap();
        let svg = format!("<svg{}", &section[..end + 6]);
        let doc = roxmltree::Document::parse(&svg).unwrap();
        if svg.contains("vertical-align") {
            assert!(doc.descendants().any(|n| n.has_tag_name("path")));
            assert!(!doc.descendants().any(|n| n.has_tag_name("text")));
            count += 1;
        }
    }
    assert_eq!(count, 4);
}

#[test]
fn multiple_mermaid_svgs_have_distinct_resolvable_fragment_ids() {
    let diagram = "```mermaid\nflowchart LR\nA --> B\n```\n";
    let dir = fixture(&format!("# Cover\n## Diagrams\n{diagram}\n{diagram}"));
    success(build(dir.path(), &[]));
    let page = html(&dir.path().join("dist"));
    let mut all_ids = std::collections::HashSet::new();
    for section in page.split("<svg").skip(1) {
        let end = section.find("</svg>").unwrap();
        let svg = format!("<svg{}", &section[..end + 6]);
        let doc = roxmltree::Document::parse(&svg).unwrap();
        let mut local_ids = std::collections::HashSet::new();
        for node in doc.descendants() {
            if let Some(id) = node.attribute("id") {
                assert!(all_ids.insert(id.to_owned()), "duplicate SVG ID {id}");
                local_ids.insert(id);
            }
        }
        for node in doc.descendants() {
            for attr in node.attributes() {
                if let Some(id) = attr
                    .value()
                    .strip_prefix("url(#")
                    .and_then(|s| s.strip_suffix(')'))
                {
                    assert!(local_ids.contains(id), "unresolved fragment {id}");
                }
            }
        }
    }
}

#[test]
fn html_inside_unknown_language_code_is_escaped() {
    let dir =
        fixture("# Cover\n## Code\n```unknown-language\n<script>alert('literal')</script>\n```\n");
    success(build(dir.path(), &[]));
    let page = html(&dir.path().join("dist"));
    assert!(page.contains("&lt;script&gt;"));
    assert!(!page.contains("<script>alert"));
}

#[test]
fn build_defaults_to_dist_and_keeps_its_output() {
    let dir = fixture("# Cover\n## Page\nHello");
    success(
        Command::new(env!("CARGO_BIN_EXE_slidown"))
            .current_dir(dir.path())
            .arg("build")
            .output()
            .unwrap(),
    );
    assert!(dir.path().join("dist/index.html").exists());
    assert_eq!(fs::read_dir(dir.path().join("dist")).unwrap().count(), 1);
    let page = html(&dir.path().join("dist"));
    let css = page
        .split("<style>\n")
        .nth(1)
        .unwrap()
        .split("\n</style>")
        .next()
        .unwrap();
    let script = page
        .split("<script>\n")
        .nth(1)
        .unwrap()
        .split("\n</script>")
        .next()
        .unwrap();
    assert!(!css.contains("/*"));
    assert!(!css.contains('\n'));
    assert!(script.len() < include_str!("../assets/slides.js").len());
    assert!(!script.contains("// Coalesce"));
    assert!(!dir.path().join("index.html").exists());
}
