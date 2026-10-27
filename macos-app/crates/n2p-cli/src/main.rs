//! `n2p` —— notebooklm2ppt 的 macOS 命令列介面。
//!
//! 與 Windows 版的差異：不再模擬滑鼠／截圖，也不需要 PowerPoint 或微軟電腦管家。
//! 可編輯文字來自 macOS 內建的 Vision OCR（或外部的 MinerU JSON），
//! PPTX 由 `n2p-ooxml` 直接產生。

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use n2p_build::BackgroundResolution;
use n2p_image::InpaintMethod;
use n2p_pipeline::{ConvertOptions, LayoutSource};
use n2p_vision::VisionConfig;

/// notebooklm2ppt: Convert PDFs to editable PowerPoint presentations.
#[derive(Debug, Parser)]
#[command(name = "n2p", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Convert a PDF to an editable PPTX
    Convert(ConvertArgs),
    /// Show PDF page count and dimensions
    Info(InfoArgs),
    /// List available watermark removal methods
    Methods,
    /// Run OCR on an image to diagnose layout results
    Ocr(OcrArgs),
    /// List candidate East Asian fonts and their installation status
    Fonts,
    /// Manage optional inference models (LaMa text removal and SAM object extraction)
    Models(ModelsArgs),
}

#[derive(Debug, Args)]
struct ModelsArgs {
    #[command(subcommand)]
    action: ModelsAction,
}

#[derive(Debug, Subcommand)]
enum ModelsAction {
    /// Download LaMa and SAM models to the cache (about 242 MiB total, once)
    Fetch,
    /// Show model status and paths
    Status,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TextModeArg {
    /// Erase original text strokes from the background, then add text boxes (default)
    Erase,
    /// Keep the background and cover original text with filled text boxes
    Cover,
    /// Keep the background and add transparent, selectable and editable text
    Overlay,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum EraseArg {
    /// Use LaMa when available (best quality), otherwise fall back to `stroke`
    #[default]
    Auto,
    /// Erase only text-colored strokes, preserving other colors such as bullets, halos and icons
    Stroke,
    /// Use LaMa to reconstruct texture under strokes and halos (run `n2p models fetch` first)
    Lama,
    /// Erase whole rectangles and interpolate from their surroundings, including any icons inside
    Inpaint,
    /// Fill with a single color, equivalent to the Python version
    Fill,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LayoutArg {
    /// Use Vision on macOS, otherwise fall back to image-only output
    Auto,
    /// macOS Vision OCR
    Vision,
    /// Use no layout information (one full-slide image per page)
    Image,
    /// Use MinerU / Paddle JSON
    Mineru,
}

#[derive(Debug, Args)]
struct ConvertArgs {
    /// Input PDF
    input: PathBuf,

    /// Output PPTX (defaults to the PDF filename with a .pptx extension)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Rasterization DPI
    #[arg(long, default_value_t = 150.0)]
    dpi: f64,

    /// Keep the NotebookLM watermark
    #[arg(long)]
    no_inpaint: bool,

    /// Watermark removal method
    #[arg(long, value_name = "ID", default_value = "background_smooth")]
    method: String,

    /// Adjust pages to 16:9
    #[arg(long)]
    widescreen: bool,

    /// Layout source
    #[arg(long, value_enum, default_value = "auto")]
    layout: LayoutArg,

    /// MinerU / Paddle JSON (required with `--layout mineru`)
    #[arg(long, value_name = "FILE")]
    mineru: Option<PathBuf>,

    /// Vision recognition languages, separated by commas
    #[arg(
        long,
        value_name = "LANGS",
        default_value = "zh-Hant,zh-Hans,ja-JP,en-US,es-ES,fr-FR"
    )]
    lang: String,

    /// Process only the specified pages, for example `1-3,5`
    #[arg(long, value_name = "RANGE")]
    pages: Option<String>,

    /// Latin font; `auto` selects a font based on the text
    #[arg(long, default_value = "auto")]
    font: String,

    /// East Asian font for CJK characters (selected automatically by default)
    #[arg(long, value_name = "NAME")]
    ea_font: Option<String>,

    /// Let PowerPoint resize text to fit its box
    #[arg(long)]
    autofit: bool,

    /// Extract separate background objects as movable images (experimental). Flat backgrounds need no model;
    /// photos or gradients require the SAM model from `n2p models fetch`
    #[arg(long)]
    extract_objects: bool,

    /// Keep the full rasterized background resolution (larger but clearer files)
    #[arg(long)]
    high_res_background: bool,

    /// Background erasure: `auto` (default) uses LaMa if available, otherwise erases strokes;
    /// `lama` forces the model; `stroke` erases strokes; `inpaint` erases rectangles; `fill` uses a solid color
    #[arg(long, value_enum, default_value = "auto")]
    erase: EraseArg,

    /// Text composition: `erase` clears text before adding boxes; `cover` uses filled boxes; `overlay` adds transparent text
    #[arg(long, value_enum, default_value = "erase")]
    text_mode: TextModeArg,

    /// Directory for intermediate images, OCR, text erasure masks and background interpolation diagnostics
    #[arg(long, value_name = "DIR")]
    work_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct InfoArgs {
    /// Input PDF
    input: PathBuf,
}

#[derive(Debug, Args)]
struct OcrArgs {
    /// Image file (PNG / JPEG)
    image: PathBuf,

    /// Recognition languages, separated by commas
    #[arg(
        long,
        value_name = "LANGS",
        default_value = "zh-Hant,zh-Hans,ja-JP,en-US,es-ES,fr-FR"
    )]
    lang: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Convert(args) => convert(args),
        Command::Info(args) => info(args),
        Command::Methods => {
            methods();
            Ok(())
        }
        Command::Ocr(args) => ocr(args),
        Command::Fonts => {
            fonts();
            Ok(())
        }
        Command::Models(args) => models(args),
    }
}

fn models(args: ModelsArgs) -> anyhow::Result<()> {
    use n2p_image::models::ModelState;
    let entries = n2p_image::models::manifest().map_err(|e| anyhow::anyhow!(e))?;
    match args.action {
        ModelsAction::Status => {
            for model in &entries {
                let name = model.name;
                match model.state() {
                    ModelState::Missing => {
                        println!("{name}: Not downloaded (run `n2p models fetch`)")
                    }
                    ModelState::Ready => println!("{name}: Ready"),
                    ModelState::Partial { size } => println!(
                        "{name}: Incomplete ({size} / {} bytes); run `n2p models fetch` again",
                        model.expected_size
                    ),
                }
                println!("  Path: {}", model.path.display());
                println!("  Source: {}", model.url);
                match model.gpu_converted() {
                    Some(true) => println!(
                        "  GPU version: Ready ({})",
                        n2p_image::gpu::converted_path(&model.path).display()
                    ),
                    Some(false) => println!(
                        "  GPU version: Not generated (converted on first use, about 1 second)"
                    ),
                    None => {}
                }
            }
            Ok(())
        }
        ModelsAction::Fetch => {
            for model in &entries {
                if model.state() == ModelState::Ready {
                    println!("Already downloaded: {}", model.path.display());
                    continue;
                }
                println!("Downloading {} -> {}", model.name, model.path.display());
                model.fetch(true).map_err(|e| anyhow::anyhow!(e))?;
                println!("Done: {}", model.path.display());
            }
            // 轉換失敗不算下載失敗：第一次載入時會再試，不行就用 CPU
            if let Err(error) = n2p_image::models::prepare_gpu(&entries) {
                eprintln!("Warning: LaMa GPU conversion failed ({error}); CPU is still available");
            }
            Ok(())
        }
    }
}

fn convert(args: ConvertArgs) -> anyhow::Result<()> {
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| args.input.with_extension("pptx"));

    let method = InpaintMethod::try_parse(&args.method).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown watermark removal method `{}`; see `n2p methods` for available values",
            args.method
        )
    })?;

    let layout = match args.layout {
        LayoutArg::Auto => LayoutSource::Auto,
        LayoutArg::Image => LayoutSource::None,
        LayoutArg::Vision => LayoutSource::Vision(vision_config(&args.lang)?),
        LayoutArg::Mineru => {
            let path = args
                .mineru
                .clone()
                .ok_or_else(|| anyhow::anyhow!("`--layout mineru` requires `--mineru <FILE>`"))?;
            LayoutSource::Mineru(path)
        }
    };

    let options = ConvertOptions {
        dpi: args.dpi,
        inpaint: !args.no_inpaint,
        inpaint_method: method,
        widescreen: args.widescreen,
        layout,
        build: n2p_build::BuildOptions {
            font_name: args.font.clone(),
            east_asian_font_name: args.ea_font.clone(),
            widescreen: args.widescreen,
            text_autofit: args.autofit,
            extract_objects: args.extract_objects,
            text_mode: match args.text_mode {
                TextModeArg::Erase => n2p_build::TextMode::Erase,
                TextModeArg::Cover => n2p_build::TextMode::Cover,
                TextModeArg::Overlay => n2p_build::TextMode::Overlay,
            },
            erase_mode: match args.erase {
                EraseArg::Auto => {
                    if n2p_image::lama::default_model_path()
                        .map(|p| p.exists())
                        .unwrap_or(false)
                    {
                        n2p_build::EraseMode::Lama
                    } else {
                        n2p_build::EraseMode::StrokeMask
                    }
                }
                EraseArg::Lama => n2p_build::EraseMode::Lama,
                EraseArg::Stroke => n2p_build::EraseMode::StrokeMask,
                EraseArg::Inpaint => n2p_build::EraseMode::Inpaint,
                EraseArg::Fill => n2p_build::EraseMode::Flat,
            },
            background_resolution: if args.high_res_background {
                BackgroundResolution::FullImage
            } else {
                BackgroundResolution::PageSize
            },
            ..Default::default()
        },
        pages: args
            .pages
            .as_deref()
            .map(n2p_pipeline::parse_page_range)
            .transpose()?,
        work_dir: args.work_dir.clone(),
    };

    println!("Input: {}", args.input.display());
    let report = n2p_pipeline::convert(&args.input, &output, &options)?;

    println!("Output: {}", output.display());
    println!("  Pages         {}", report.pages);
    println!("  Canvas        {:?}", report.slide_size);
    println!("  Layout        {}", report.layout_name);
    println!("  Text boxes    {}", report.text_boxes);
    println!("  Pictures      {}", report.pictures);
    println!("  Cleaned pages {}", report.watermarks_removed);
    println!("  File size     {} bytes", report.output_bytes);
    Ok(())
}

fn info(args: InfoArgs) -> anyhow::Result<()> {
    let doc = n2p_pdf::PdfDoc::open(&args.input)?;
    println!("{}", args.input.display());
    println!("  Pages   {}", doc.page_count());
    for index in 0..doc.page_count() {
        let (width, height) = doc.page_size_pt(index)?;
        let (px_w, px_h) = doc.pixel_size(index, 150.0)?;
        println!(
            "  page {}  {width:.2} x {height:.2} pt ({:.1} x {:.1} in) -> {px_w} x {px_h} px @ 150 dpi",
            index + 1,
            width / 72.0,
            height / 72.0
        );
    }
    Ok(())
}

fn methods() {
    println!("Available watermark removal methods (`--method`):");
    for method in InpaintMethod::all() {
        let note = if method.is_bit_exact_with_python() {
            "Bit-exact with the Python version"
        } else {
            "Alternative algorithm with similar visual results"
        };
        println!("  {:<18} {} - {}", method.id(), method.display_name(), note);
        println!("  {:<18} {}", "", method.description());
    }
}

fn fonts() {
    println!("Candidate East Asian fonts (in preference order; the first installed font is used):");
    let mut picked = false;
    for (family, installed) in n2p_build::fonts::availability() {
        let mark = if installed {
            "[installed]"
        } else {
            "[not installed]"
        };
        let tag = if installed && !picked {
            picked = true;
            "  <- selected"
        } else {
            ""
        };
        println!("  {mark}  {family}{tag}");
    }
    if !picked {
        println!(
            "\n  No candidate fonts found; <a:ea> will be omitted and PowerPoint will use the theme font."
        );
    }
    println!("\nUse --font / --ea-font to specify other fonts.");
}

fn ocr(args: OcrArgs) -> anyhow::Result<()> {
    let image = n2p_core::RgbImage::open(&args.image)?;
    let config = vision_config(&args.lang)?;
    println!(
        "{}: {} x {}",
        args.image.display(),
        image.width,
        image.height
    );

    let observations = n2p_vision::recognize(&image, &config)?;
    println!("Recognized {} observations:", observations.len());
    for (index, observation) in observations.iter().enumerate() {
        let (x, y, w, h) = observation.bbox_norm;
        println!(
            "  [{index}] conf={:.2} bbox=({x:.3}, {y:.3}, {w:.3}, {h:.3}) {:?}",
            observation.confidence, observation.text
        );
    }
    Ok(())
}

fn vision_config(langs: &str) -> anyhow::Result<VisionConfig> {
    let languages: Vec<String> = langs
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    if languages.is_empty() {
        return Err(anyhow::anyhow!(
            "At least one recognition language is required"
        ));
    }
    Ok(VisionConfig {
        languages,
        ..Default::default()
    })
}
