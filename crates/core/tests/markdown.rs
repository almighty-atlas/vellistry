use mdwell_core::{Core, ResolvedLink};
use std::fs;

#[test]
fn frontmatter_is_hidden_without_changing_source_or_byte_offsets() {
    for (prefix, newline) in [("", "\n"), ("\u{feff}", "\r\n")] {
        for closing in ["---", "..."] {
            let data = tempfile::tempdir().unwrap();
            let files = tempfile::tempdir().unwrap();
            let source = format!("{prefix}---{newline}title: Secretmetadata{newline}description: café{newline}{closing}{newline}# Visible{newline}{newline}A naïve navigation paragraph.{newline}");
            fs::write(files.path().join("doc.md"), &source).unwrap();
            let core = Core::open(data.path()).unwrap();
            let ws = core.add_local(files.path()).unwrap();
            let doc = core.read_document(ws.id, "doc.md").unwrap();
            assert_eq!(doc.content, source);
            assert_eq!(doc.title, "Visible");
            assert!(!doc.html.contains("Secretmetadata"));
            assert!(!doc.html.contains("description:"));
            assert!(!doc.html.contains("<hr"));
            assert_eq!(doc.toc[0].source_start, source.find("# Visible").unwrap());
            let start = source.find("A naïve").unwrap();
            assert!(doc.html.contains(&format!("data-source-start=\"{start}\"")));
            let hit = core.search("naive", Some(ws.id)).unwrap().remove(0);
            assert_eq!(hit.source_start, start);
            assert!(core
                .search("Secretmetadata", Some(ws.id))
                .unwrap()
                .is_empty());
            let saved = core
                .save_document(ws.id, "doc.md", &source, &doc.revision)
                .unwrap();
            assert_eq!(saved.content, source);
            assert_eq!(saved.revision, doc.revision);
            assert_eq!(
                fs::read(files.path().join("doc.md")).unwrap(),
                source.as_bytes()
            );
        }
    }
}

#[test]
fn only_closed_leading_frontmatter_is_hidden() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let core = Core::open(data.path()).unwrap();
    let cases = [
        (
            "unclosed.md",
            "---\nUnclosedmetadata\n# Visible",
            "Unclosedmetadata",
        ),
        (
            "body.md",
            "# Visible\n\n---\nBodymetadata\n---\n",
            "Bodymetadata",
        ),
        ("empty.md", "---\nsecret: hidden\n---", ""),
    ];
    for (path, source, _) in cases {
        fs::write(files.path().join(path), source).unwrap();
    }
    let ws = core.add_local(files.path()).unwrap();
    for (path, _, visible) in cases {
        let doc = core.read_document(ws.id, path).unwrap();
        if visible.is_empty() {
            assert!(doc.html.is_empty());
            assert!(doc.toc.is_empty());
        } else {
            assert!(doc.html.contains(visible));
        }
    }
}

#[test]
fn markdown_images_are_inert_escaped_and_resolvable_but_raw_html_is_not_trusted() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    fs::create_dir(files.path().join("nested")).unwrap();
    fs::create_dir(files.path().join("assets")).unwrap();
    fs::write(
        files.path().join("assets/local image.png"),
        b"raster fixture",
    )
    .unwrap();
    let source = "# Images\n\n![A *local* `image` & \"quoted\"](<../assets/local image.png>)\n\n![x&quot; onerror=&quot;alert(1)](safe.png)\n\n![linked](a&amp;b.png)\n\n![remote](https://tracker.invalid/pixel.png)\n\n![protocol](//tracker.invalid/pixel.png)\n\n![absolute](/etc/private.png)\n\n![windows](C:/private.png)\n\n![data](data:image/png;base64,AAAA)\n\n![script](javascript:alert%281%29)\n\n![file](file:///etc/private.png)\n\n<img data-local-src=\"forged.png\" src=\"https://tracker.invalid/raw.png\" onerror=\"alert(1)\">\n\n<p style=\"color:red\">Rawparagraph</p>\n\nInline <img src=\"inline.png\" data-local-src=\"inline.png\"> text.\n";
    fs::write(files.path().join("nested/doc.md"), source).unwrap();
    let core = Core::open(data.path()).unwrap();
    let ws = core.add_local(files.path()).unwrap();
    let doc = core.read_document(ws.id, "nested/doc.md").unwrap();
    assert_eq!(doc.html.matches("<img ").count(), 3);
    assert!(doc
        .html
        .contains("data-local-src=\"../assets/local image.png\""));
    assert!(doc
        .html
        .contains("alt=\"A local image &amp; &quot;quoted&quot;\""));
    assert!(doc.html.contains("alt=\"x&quot; onerror=&quot;alert(1)\""));
    assert!(doc.html.contains("data-local-src=\"a&amp;b.png\""));
    for forbidden in [
        " src=",
        " srcset=",
        " onerror=\"",
        " style=",
        "tracker.invalid",
        "data:image",
        "javascript:",
        "file:///",
        "forged.png",
        "inline.png",
        "Rawparagraph",
        "/etc/private",
        "C:/private",
    ] {
        assert!(
            !doc.html.contains(forbidden),
            "unexpected {forbidden}: {}",
            doc.html
        );
    }
    assert!(doc.html.contains("Inline"));
    let link = core
        .resolve_link(ws.id, "nested/doc.md", "../assets/local image.png")
        .unwrap();
    let ResolvedLink::Asset { path, .. } = link else {
        panic!("expected local asset");
    };
    assert_eq!(
        core.read_asset(ws.id, &path).unwrap().bytes,
        b"raster fixture"
    );
    let start = source.find("![A").unwrap();
    assert!(doc.html.contains(&format!("data-source-start=\"{start}\"")));
}

#[test]
fn gfm_admonition_classes_survive_sanitization_without_raw_html_or_styles() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let mut source = String::from("# Alerts\n\n");
    for kind in ["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"] {
        source.push_str(&format!("> [!{kind}]\n> Content for {kind}.\n\n"));
    }
    source.push_str("<blockquote class=\"forged\" style=\"background:url(https://tracker.invalid)\">raw</blockquote>\n");
    fs::write(files.path().join("doc.md"), &source).unwrap();
    let core = Core::open(data.path()).unwrap();
    let ws = core.add_local(files.path()).unwrap();
    let doc = core.read_document(ws.id, "doc.md").unwrap();
    assert_eq!(doc.html.matches("<blockquote").count(), 5);
    for kind in ["note", "tip", "important", "warning", "caution"] {
        assert!(
            doc.html.contains(&format!("markdown-alert-{kind}")),
            "{}",
            doc.html
        );
    }
    assert!(!doc.html.contains("[!NOTE]"));
    assert!(!doc.html.contains("forged"));
    assert!(!doc.html.contains("style="));
    assert!(!doc.html.contains("tracker.invalid"));
    let start = source.find("> [!NOTE]").unwrap();
    assert!(doc.html.contains(&format!("data-source-start=\"{start}\"")));
}
