use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "n2p-cli-english-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_n2p"));
        command
            .current_dir(&self.0)
            .env("N2P_LAMA_MODEL", self.0.join("lama.onnx"))
            .env("N2P_SAM_DIR", self.0.join("sam"))
            .env("LC_ALL", "zh_TW.UTF-8");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn all_help_is_english_even_in_a_chinese_locale() {
    let sandbox = Sandbox::new();
    for args in [
        vec!["--help"],
        vec!["convert", "--help"],
        vec!["info", "--help"],
        vec!["methods", "--help"],
        vec!["ocr", "--help"],
        vec!["fonts", "--help"],
        vec!["models", "--help"],
        vec!["models", "fetch", "--help"],
        vec!["models", "status", "--help"],
    ] {
        let output = sandbox.run(&args);
        assert!(output.status.success(), "{args:?}: {}", stderr(&output));
        let text = stdout(&output);
        assert!(text.contains("Usage:"), "{text}");
        assert!(text.is_ascii(), "{args:?}: {text}");
        assert!(output.stderr.is_empty());
    }
    let fetch = stdout(&sandbox.run(&["models", "fetch", "--help"]));
    assert!(fetch.contains("LaMa") && fetch.contains("SAM"), "{fetch}");
}

#[test]
fn method_names_and_descriptions_are_english() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["methods"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(
        text.contains("Available watermark removal methods"),
        "{text}"
    );
    assert!(text.contains("background_smooth"));
    assert!(text.contains("Smart smoothing"));
    assert!(text.is_ascii(), "{text}");
}

#[test]
fn font_status_labels_are_english_and_font_names_are_preserved() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["fonts"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("Candidate East Asian fonts"), "{text}");
    assert!(text.contains("Use --font / --ea-font"));
    for (family, installed) in n2p_build::fonts::availability() {
        let status = if installed {
            "[installed]"
        } else {
            "[not installed]"
        };
        assert!(text.contains(&format!("{status}  {family}")), "{text}");
    }
}

#[test]
fn model_status_reports_missing_partial_and_ready_in_english() {
    let sandbox = Sandbox::new();
    let missing = sandbox.run(&["models", "status"]);
    assert!(missing.status.success());
    assert!(stdout(&missing).contains("Not downloaded"));
    assert!(stdout(&missing).is_ascii(), "{}", stdout(&missing));

    std::fs::write(sandbox.0.join("lama.onnx"), b"short").unwrap();
    let partial = sandbox.run(&["models", "status"]);
    assert!(partial.status.success());
    assert!(stdout(&partial).contains("Incomplete (5 / 208044816 bytes)"));

    for model in n2p_image::models::manifest().unwrap() {
        let path = match model.purpose {
            n2p_image::models::Purpose::Erase => sandbox.0.join("lama.onnx"),
            n2p_image::models::Purpose::Objects => {
                sandbox.0.join("sam").join(model.path.file_name().unwrap())
            }
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::File::create(&path)
            .unwrap()
            .set_len(model.expected_size)
            .unwrap();
    }
    // A cached conversion keeps this display test independent of model inference.
    std::fs::write(
        n2p_image::gpu::converted_path(&sandbox.0.join("lama.onnx")),
        b"cached",
    )
    .unwrap();
    let ready = sandbox.run(&["models", "status"]);
    assert!(ready.status.success());
    assert!(stdout(&ready).contains(": Ready"));
    assert!(stdout(&ready).is_ascii(), "{}", stdout(&ready));
    let fetch = sandbox.run(&["models", "fetch"]);
    assert!(fetch.status.success(), "{}", stderr(&fetch));
    assert_eq!(stdout(&fetch).matches("Already downloaded").count(), 3);
    assert!(stdout(&fetch).is_ascii());
}

#[test]
fn invalid_arguments_and_conversion_errors_are_english() {
    let sandbox = Sandbox::new();
    for (args, expected) in [
        (
            vec!["convert", "missing.pdf", "--method", "nope"],
            "Unknown watermark removal method",
        ),
        (
            vec!["convert", "missing.pdf", "--layout", "mineru"],
            "requires `--mineru <FILE>`",
        ),
        (
            vec![
                "convert",
                "missing.pdf",
                "--layout",
                "vision",
                "--lang",
                ", ,",
            ],
            "At least one recognition language",
        ),
        (
            vec!["convert", "missing.pdf", "--pages", "0"],
            "Page numbers start at 1",
        ),
        (
            vec!["convert", "missing.pdf", "--dpi", "0"],
            "DPI must be greater than 0",
        ),
        (
            vec!["convert", "missing.pdf"],
            "PDF processing failed: I/O failed:",
        ),
        (vec!["info", "missing.pdf"], "I/O failed:"),
    ] {
        let output = sandbox.run(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let text = stderr(&output);
        assert!(text.starts_with("Error:"), "{text}");
        assert!(text.contains(expected), "{args:?}: {text}");
        assert!(text.is_ascii(), "{text}");
    }
    let output = sandbox.run(&["convert", "missing.pdf", "--layout", "unknown"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("invalid value"));
    assert!(stderr(&output).is_ascii());
}

#[test]
fn image_errors_and_ocr_headings_are_english() {
    let sandbox = Sandbox::new();
    std::fs::write(sandbox.0.join("invalid.png"), b"invalid image").unwrap();
    let invalid = sandbox.run(&["ocr", "invalid.png"]);
    assert_eq!(invalid.status.code(), Some(1));
    assert!(stderr(&invalid).contains("Error: PNG decoding failed:"));
    assert!(stderr(&invalid).is_ascii());

    let image = fixture("test.png");
    let empty = sandbox.run(&["ocr", image.to_str().unwrap(), "--lang", ", ,"]);
    assert_eq!(empty.status.code(), Some(1));
    assert!(stderr(&empty).contains("At least one recognition language"));

    #[cfg(target_os = "macos")]
    {
        let output = sandbox.run(&["ocr", image.to_str().unwrap(), "--lang", "en-US"]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(
            stdout(&output).contains("Recognized "),
            "{}",
            stdout(&output)
        );
    }
}

#[test]
fn conversion_messages_are_english_and_user_text_is_preserved() {
    let sandbox = Sandbox::new();
    let input = sandbox.0.join("日本語-中文.pdf");
    std::fs::copy(fixture("floyd.pdf"), &input).unwrap();
    let pdf = n2p_pdf::PdfDoc::open(&input).unwrap();
    let (w, h) = pdf.page_size_pt(0).unwrap();
    let text = "日本語と中文原稿";
    let layout = sandbox.0.join("layout.json");
    std::fs::write(&layout, format!(
        r#"{{"pdf_info":[{{"page_size":[{w},{h}],"para_blocks":[{{"type":"text","bbox":[10,10,200,40],"lines":[{{"spans":[{{"content":"{text}"}}]}}]}}]}},{{"page_size":[{w},{h}],"para_blocks":[]}}]}}"#
    )).unwrap();
    let output_path = sandbox.0.join("出力-输出.pptx");
    let output = sandbox
        .command()
        .args([
            "convert",
            input.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--layout",
            "mineru",
            "--mineru",
            layout.to_str().unwrap(),
            "--pages",
            "1",
            "--no-inpaint",
            "--dpi",
            "36",
            "--erase",
            "lama",
            "--method",
            "逐層內縮修補",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let result = stdout(&output);
    assert!(
        result.contains(&format!("Input: {}", input.display())),
        "{result}"
    );
    assert!(result.contains(&format!("Output: {}", output_path.display())));
    assert!(result.contains("Text boxes    1"), "{result}");
    assert!(
        result
            .replace(input.to_str().unwrap(), "INPUT")
            .replace(output_path.to_str().unwrap(), "OUTPUT")
            .is_ascii()
    );
    let warning = stderr(&output);
    assert!(warning.contains("Warning: LaMa unavailable"), "{warning}");
    assert!(warning.contains("LaMa model not found"), "{warning}");
    assert!(warning.is_ascii(), "{warning}");
    let deck = n2p_ooxml::Deck::open(&output_path).unwrap();
    assert_eq!(deck.slide_count().unwrap(), 1);
    let slide = n2p_ooxml::xml::to_bytes(&deck.slide(0).unwrap()).unwrap();
    assert!(String::from_utf8(slide).unwrap().contains(text));

    let info = sandbox.run(&["info", input.to_str().unwrap()]);
    assert!(info.status.success());
    assert!(stdout(&info).contains("Pages"));
    assert!(stdout(&info).contains(input.to_str().unwrap()));

    let unknown = sandbox.run(&["convert", "missing.pdf", "--method", "未知-日本語"]);
    assert!(stderr(&unknown).contains("Unknown watermark removal method `未知-日本語`"));
}

#[test]
fn malformed_layout_errors_reach_the_cli_in_english() {
    let sandbox = Sandbox::new();
    std::fs::write(sandbox.0.join("layout.json"), "{}").unwrap();
    let output = sandbox.run(&[
        "convert",
        fixture("floyd.pdf").to_str().unwrap(),
        "--layout",
        "mineru",
        "--mineru",
        "layout.json",
        "--no-inpaint",
        "--dpi",
        "12",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let error = stderr(&output);
    assert!(
        error.contains("Error: Layout parsing failed: Malformed layout data:"),
        "{error}"
    );
    assert!(error.contains("pdf_info"));
    assert!(error.is_ascii(), "{error}");
}
