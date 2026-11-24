use base64::{Engine, engine::general_purpose::STANDARD};
use lopdf::{Document, Object, ObjectId};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

const PNG: &[u8] = include_bytes!("fixtures/pixel.png");
const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"200\" height=\"100\"><rect width=\"200\" height=\"100\" fill=\"#38a6a0\"/></svg>";
const THEMES: &[&str] = &["ocean", "teal", "forest", "plum", "terracotta", "graphite"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn binary() -> PathBuf {
    std::env::var_os("MDINK_BIN")
        .map(|p| fs::canonicalize(p).unwrap())
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_mdink")))
}
fn command() -> Command {
    Command::new(binary())
}
fn run(args: &[&str]) -> Output {
    command().args(args).output().unwrap()
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn convert(input: &Path, args: &[&str]) -> Document {
    success(command().arg(input).args(args).output().unwrap());
    Document::load(input.with_extension("pdf")).unwrap()
}
fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn text(pdf: &Document, pages: &[u32]) -> String {
    normalized(&pdf.extract_text(pages).unwrap())
}
fn all_text(pdf: &Document) -> String {
    text(pdf, &pdf.get_pages().keys().copied().collect::<Vec<_>>())
}
fn object<'a>(pdf: &'a Document, obj: &'a Object) -> &'a Object {
    pdf.dereference(obj).unwrap().1
}
fn dimensions(pdf: &Document, page: ObjectId) -> (f32, f32) {
    let rect = object(
        pdf,
        pdf.get_dictionary(page).unwrap().get(b"MediaBox").unwrap(),
    )
    .as_array()
    .unwrap();
    (
        rect[2].as_float().unwrap() - rect[0].as_float().unwrap(),
        rect[3].as_float().unwrap() - rect[1].as_float().unwrap(),
    )
}
fn file_url(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    const SET: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
        .add(b' ')
        .add(b'#')
        .add(b'?')
        .add(b'%');
    format!(
        "file://{}{}",
        if path.starts_with('/') { "" } else { "/" },
        percent_encoding::utf8_percent_encode(&path, SET)
    )
}

#[derive(Debug)]
struct TextRun {
    text: String,
    x: f32,
    y: f32,
    width: f32,
}

// Inspect actual PDF text operators, font encodings and CID widths independently of the renderer.
fn text_runs(pdf: &Document, page: ObjectId) -> Vec<TextRun> {
    let fonts = pdf.get_page_fonts(page).unwrap();
    let mut widths = HashMap::new();
    for (name, font) in &fonts {
        let descendants = object(pdf, font.get(b"DescendantFonts").unwrap())
            .as_array()
            .unwrap();
        let cid = object(pdf, &descendants[0]).as_dict().unwrap();
        let entries = object(pdf, cid.get(b"W").unwrap()).as_array().unwrap();
        let mut map = HashMap::new();
        let mut i = 0;
        while i < entries.len() {
            let first = entries[i].as_i64().unwrap() as u16;
            if let Ok(array) = entries[i + 1].as_array() {
                for (j, width) in array.iter().enumerate() {
                    map.insert(first + j as u16, width.as_float().unwrap());
                }
                i += 2;
            } else {
                let last = entries[i + 1].as_i64().unwrap() as u16;
                let width = entries[i + 2].as_float().unwrap();
                for code in first..=last {
                    map.insert(code, width);
                }
                i += 3;
            }
        }
        widths.insert(name.clone(), map);
    }
    let content = pdf.get_and_decode_page_content(page).unwrap();
    let mut font_name = Vec::new();
    let (mut size, mut x, mut y) = (0.0, 0.0, 0.0);
    let mut result = Vec::new();
    for op in content.operations {
        match op.operator.as_str() {
            "Tf" => {
                font_name = op.operands[0].as_name().unwrap().to_vec();
                size = op.operands[1].as_float().unwrap();
            }
            "Tm" => {
                x = op.operands[4].as_float().unwrap();
                y = op.operands[5].as_float().unwrap();
            }
            "Tj" | "TJ" => {
                let enc = fonts[&font_name].get_font_encoding(pdf).unwrap();
                let items = if op.operator == "TJ" {
                    op.operands[0].as_array().unwrap().clone()
                } else {
                    op.operands
                };
                let mut text = String::new();
                let mut width = 0.0;
                for item in items {
                    if let Ok(bytes) = item.as_str() {
                        text.push_str(&Document::decode_text(&enc, bytes).unwrap());
                        for code in bytes.as_chunks::<2>().0 {
                            let code = u16::from_be_bytes([code[0], code[1]]);
                            width += widths[&font_name].get(&code).copied().unwrap_or(1000.0)
                                * size
                                / 1000.0;
                        }
                    } else {
                        width -= item.as_float().unwrap() * size / 1000.0;
                    }
                }
                result.push(TextRun { text, x, y, width });
                x += width;
            }
            _ => {}
        }
    }
    result
}

#[test]
fn standalone_binary_formats_sample_with_embedded_fonts_and_links() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("sample.md");
    fs::write(&input, include_str!("fixtures/document.md")).unwrap();
    fs::write(dir.path().join("pixel.png"), PNG).unwrap();
    let output = dir.path().join("sub/sample.pdf");
    success(
        command()
            .arg(&input)
            .arg("-o")
            .arg(&output)
            .args(["--title", "Test report"])
            .current_dir(dir.path())
            .env_clear()
            .env("PATH", "")
            .output()
            .unwrap(),
    );
    let pdf = Document::load(output).unwrap();
    let info = object(&pdf, pdf.trailer.get(b"Info").unwrap())
        .as_dict()
        .unwrap();
    assert_eq!(
        lopdf::decode_text_string(info.get(b"Title").unwrap()).unwrap(),
        "Test report"
    );
    assert_eq!(
        lopdf::decode_text_string(info.get(b"Creator").unwrap()).unwrap(),
        "mdink (native)"
    );
    let content = all_text(&pdf);
    for expected in [
        "From ideas",
        "Formatting",
        "Hello, PDF!",
        "città",
        "no syntax highlighting",
        "final footnote",
        "[1]",
    ] {
        assert!(content.contains(expected), "Missing {expected}: {content}");
    }
    let mut fonts = BTreeSet::new();
    let mut links = Vec::new();
    for page in pdf.get_pages().values() {
        let (w, h) = dimensions(&pdf, *page);
        assert!((w - 595.28).abs() < 1.0 && (h - 841.89).abs() < 1.0);
        for font in pdf.get_page_fonts(*page).unwrap().values() {
            fonts.insert(font.get(b"BaseFont").unwrap().as_name().unwrap().to_vec());
        }
        for annotation in pdf.get_page_annotations(*page).unwrap() {
            let action = object(&pdf, annotation.get(b"A").unwrap())
                .as_dict()
                .unwrap();
            links.push(action.get(b"URI").unwrap().as_str().unwrap().to_vec());
        }
    }
    assert!(fonts.len() >= 5);
    assert!(
        pdf.objects
            .values()
            .filter_map(|o| o.as_dict().ok())
            .filter(|d| d.has(b"FontFile2"))
            .count()
            >= 5
    );
    assert!(links.iter().any(|uri| uri == b"https://commonmark.org/"));
}

#[test]
fn br_tags_break_paragraphs_and_tables_but_remain_literal_in_code() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("breaks.md");
    fs::write(&input, "first<br />second<br/>third<BR>fourth\n\n| Cell |\n|---|\n| above<br />below |\n\n`<br />`\n\n```html\n<br />\n```\n\n<b>text</b>").unwrap();
    let pdf = convert(&input, &[]);
    let runs = text_runs(&pdf, pdf.get_pages()[&1]);
    for (before, after) in [
        ("first", "second"),
        ("second", "third"),
        ("third", "fourth"),
        ("above", "below"),
    ] {
        let a = runs.iter().find(|r| r.text == before).unwrap();
        let b = runs.iter().find(|r| r.text == after).unwrap();
        assert!((a.y - b.y).abs() > 1.0, "Expected a new line: {a:?}, {b:?}");
    }
    let content = all_text(&pdf);
    assert_eq!(content.matches("<br />").count(), 2);
    assert!(
        runs.iter()
            .map(|r| r.text.as_str())
            .collect::<String>()
            .contains("<b>text</b>")
    );
}

#[test]
fn six_themes_and_custom_colors_change_pdf_palette() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("themes.md");
    fs::write(
        &input,
        "# Colors\n\n| Header |\n|---|\n| Value |\n\n> Quote\n\n```\ncode\n```",
    )
    .unwrap();
    let list = success(run(&["--list-themes"]));
    for theme in THEMES {
        assert!(list.contains(theme));
    }
    let mut palettes = BTreeSet::new();
    for theme in THEMES
        .iter()
        .copied()
        .chain(["#ff6600", "#ffffff", "#000000"])
    {
        let pdf = convert(
            &input,
            &[
                if theme.starts_with('#') {
                    "--color"
                } else {
                    "--theme"
                },
                theme,
            ],
        );
        let ops = pdf
            .get_and_decode_page_content(pdf.get_pages()[&1])
            .unwrap();
        let colors: Vec<_> = ops
            .operations
            .iter()
            .filter(|op| matches!(op.operator.as_str(), "rg" | "scn" | "g"))
            .map(|op| format!("{:?}", op.operands))
            .collect();
        assert!(colors.iter().collect::<BTreeSet<_>>().len() >= 4);
        assert!(palettes.insert(colors), "Duplicate palette: {theme}");
    }
    for args in [
        ["--theme", "unknown"],
        ["--color", "#xx22xx"],
        ["--color", "white"],
    ] {
        assert!(
            !command()
                .arg(&input)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
}

struct Server {
    origin: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut buffer = [0; 4096];
                        let n = stream.read(&mut buffer).unwrap_or(0);
                        let found = buffer[..n].starts_with(b"GET /image.png ");
                        let (status, body) = if found {
                            ("200 OK", PNG)
                        } else {
                            ("404 Not Found", &b"missing"[..])
                        };
                        let header = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream
                            .write_all(header.as_bytes())
                            .and_then(|_| stream.write_all(body));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            origin,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn assert_images(pdf: &Document, count: usize) {
    let page = pdf.get_pages()[&1];
    assert!(!pdf.get_page_images(page).unwrap().is_empty());
    let ops = pdf.get_and_decode_page_content(page).unwrap();
    assert!(
        ops.operations
            .iter()
            .filter(|op| op.operator == "Do")
            .count()
            >= count
    );
}

#[test]
fn local_http_file_and_data_images_preserve_previous_output_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::new();
    let input = dir.path().join("images.md");
    let image = dir.path().join("pretty image.png");
    fs::write(&image, PNG).unwrap();
    fs::write(dir.path().join("image.svg"), SVG).unwrap();
    fs::write(&input, format!("# Images\n\n![local](<pretty image.png>)\n\n![remote]({}/image.png)\n\n![file]({})\n\n![data](data:image/png;base64,{})\n\n![svg](image.svg)", server.origin, file_url(&image), STANDARD.encode(PNG))).unwrap();
    let pdf = convert(&input, &[]);
    assert_images(&pdf, 4);
    let previous = fs::read(input.with_extension("pdf")).unwrap();
    for src in [
        format!("{}/missing.png", server.origin),
        "missing.png".into(),
    ] {
        fs::write(&input, format!("![broken]({src})")).unwrap();
        let output = command().arg(&input).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Unable to load"));
        assert_eq!(fs::read(input.with_extension("pdf")).unwrap(), previous);
    }
}

#[test]
fn multipage_tables_oversized_cells_and_code_preserve_text_without_clipping() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("long.md");
    let rows = (0..100)
        .map(|i| format!("| Row {i} | data {i} |\n"))
        .collect::<String>();
    fs::write(&input, format!("# Report\n\n| Header | Value |\n|---|---|\n{rows}| Start {}END CELL | last value |\n\n```\n{}END CODE\n{}\n```\n\nEND DOCUMENT", "CONTENT ".repeat(1200), "code line\n".repeat(100), "x".repeat(500))).unwrap();
    let pdf = convert(&input, &["--landscape"]);
    let pages = pdf.get_pages();
    assert!(pages.len() >= 3);
    for (number, page) in &pages {
        let (width, height) = dimensions(&pdf, *page);
        assert!(width > height);
        let content = text(&pdf, &[*number]);
        if content.contains("Row") || content.contains("CONTENT") {
            assert!(content.contains("Header"));
        }
        for run in text_runs(&pdf, *page) {
            if run.text.trim().is_empty() {
                continue;
            }
            assert!(
                run.x >= 52.0 && run.x + run.width <= width - 50.0,
                "Horizontal clipping: {run:?}"
            );
            assert!(
                run.y.abs() >= 24.0 && run.y.abs() <= height - 24.0,
                "Vertical clipping: {run:?}"
            );
        }
    }
    let content = all_text(&pdf);
    for expected in ["Row 99", "END CELL", "END CODE", "END DOCUMENT"] {
        assert!(content.contains(expected), "Missing {expected}");
    }
    assert_eq!(content.matches("CONTENT").count(), 1200);
    assert_eq!(content.matches('x').count(), 500);
}

#[test]
fn jpeg_gif_webp_and_bmp_codecs() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("formats.md");
    let source = ["jpg", "gif", "webp", "bmp"]
        .map(|ext| {
            format!(
                "![{ext}]({})",
                file_url(&root().join(format!("tests/fixtures/pixel.{ext}")))
            )
        })
        .join("\n\n");
    fs::write(&input, source).unwrap();
    assert_images(&convert(&input, &[]), 4);
}

#[test]
fn cli_help_version_errors_and_output_protection_are_english() {
    let dir = tempfile::tempdir().unwrap();
    assert!(success(run(&["--help"])).contains("Usage: mdink"));
    assert_eq!(success(run(&["--version"])).trim(), "1.0.0");
    let input = dir.path().join("input.md");
    fs::write(&input, "# Test").unwrap();
    assert!(success(command().arg(&input).output().unwrap()).starts_with("PDF created:"));
    for (args, error) in [
        (["--theme", "unknown"], "Unknown theme:"),
        (["--color", "invalid"], "Invalid color:"),
    ] {
        let output = command().arg(&input).args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(error));
    }
    for args in [
        vec![],
        vec!["--unknown"],
        vec!["missing.md"],
        vec!["one.md", "two.md"],
        vec!["--output"],
        vec!["--color"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).starts_with("Error:"));
    }
    for output in [input.clone(), dir.path().join("out.txt")] {
        assert!(
            !command()
                .arg(&input)
                .arg("-o")
                .arg(output)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    assert_eq!(fs::read_to_string(input).unwrap(), "# Test");
}
