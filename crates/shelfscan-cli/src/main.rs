//! Demo for the shelfscan library: scan -> index -> review -> draft listings -> CSV (and optionally Shopify drafts).
//!
//! ```text
//! shelfscan --source synthetic --out out/
//! shelfscan --source images --input ./photos --review
//! shelfscan --source ffmpeg --input-format avfoundation --input 0 --seconds 30 --review   # Mac webcam (UNVERIFIED)
//! shelfscan fixtures --out fixtures/                                                      # test PNGs for Swift / Kotlin
//! ```
//! Cloud is opt-in: `--anthropic` (reads ANTHROPIC_API_KEY), `--off-contact you@example.com`,
//! `--shopify-shop my-store` (reads SHOPIFY_ADMIN_TOKEN, or SHOPIFY_CLIENT_ID + SHOPIFY_CLIENT_SECRET; creates DRAFT products only).

use std::{
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use shelfscan::*;

mod fixtures;

#[derive(Copy, Clone, ValueEnum)]
enum Source {
    /// Built-in synthetic tabletop with three products.
    Synthetic,
    /// A folder of .jpg/.png photos, one view each (sorted by name).
    Images,
    /// Raw RGB frames piped from `ffmpeg` (file, webcam, anything ffmpeg can read).
    Ffmpeg,
}

#[derive(Copy, Clone, ValueEnum)]
enum ModeArg {
    /// Catalog mode: barcode is ground truth.
    Barcode,
    /// Handmade mode: describe from images.
    Describe,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write deterministic test fixtures (PNGs + fixtures.json) for tests in Swift, Kotlin or Python.
    Fixtures {
        #[arg(long, default_value = "fixtures")]
        out: PathBuf,
    },
}

#[derive(Parser)]
#[command(name = "shelfscan", version, about = "Scan a table or shelf, index the products, draft one listing each.", args_conflicts_with_subcommands = true)]
struct Args {
    #[command(subcommand)]
    command: Option<Cmd>,
    #[arg(long, value_enum, default_value = "synthetic")]
    source: Source,
    /// Folder (images), file or device (ffmpeg).
    #[arg(long)]
    input: Option<String>,
    /// ffmpeg input format, e.g. `avfoundation` on a Mac or `v4l2` on Linux.
    #[arg(long)]
    input_format: Option<String>,
    /// Extra ffmpeg input options placed before `-i`, e.g. `--input-arg=-framerate --input-arg=30`.
    #[arg(long = "input-arg", allow_hyphen_values = true)]
    input_args: Vec<String>,
    /// Frame size delivered to the engine by ffmpeg (WIDTHxHEIGHT).
    #[arg(long, default_value = "640x360")]
    size: String,
    #[arg(long, default_value_t = 6.0)]
    fps: f32,
    /// How long to record from ffmpeg.
    #[arg(long, default_value_t = 20)]
    seconds: u32,
    /// Times each photo is shown to the tracker (a track needs a few sightings to become a product).
    #[arg(long, default_value_t = 3)]
    repeat: u32,
    #[arg(long, default_value = "out")]
    out: PathBuf,
    #[arg(long, value_enum, default_value = "barcode")]
    mode: ModeArg,
    /// Walk through the items and confirm, correct or hide them before drafting.
    #[arg(long)]
    review: bool,
    /// Keep scans, SKUs and the fixture library in this SQLite file.
    #[arg(long)]
    sqlite: Option<PathBuf>,
    #[arg(long, default_value = "SKU")]
    sku_prefix: String,
    /// Identify products and write copy with the Anthropic API (masked crops only). Needs ANTHROPIC_API_KEY.
    #[arg(long)]
    anthropic: bool,
    #[arg(long)]
    model: Option<String>,
    /// Look barcodes up in Open Food Facts; the contact goes into the User-Agent.
    #[arg(long)]
    off_contact: Option<String>,
    /// Also create DRAFT products in this Shopify store. Needs SHOPIFY_ADMIN_TOKEN or SHOPIFY_CLIENT_ID + SHOPIFY_CLIENT_SECRET.
    #[arg(long)]
    shopify_shop: Option<String>,
}

struct Printer;

impl EventListener for Printer {
    fn on_event(&self, e: Event) {
        match e {
            Event::ItemDiscovered { item_id, .. } => eprintln!("  + found {item_id}"),
            Event::GuessReady { item_id, title, confidence } => eprintln!("  ? {item_id}: {title} ({:.0}%)", confidence * 100.0),
            Event::BarcodeRead { item_id, gtin } => eprintln!("  # {item_id}: barcode {gtin}"),
            Event::ItemMerged { from_id, into_id } => eprintln!("  = {from_id} is the same product as {into_id}"),
            Event::PossibleDuplicate { item_id, existing_id, similarity } => eprintln!("  ~ {item_id} looks like earlier item {existing_id} ({:.0}%)", similarity * 100.0),
            Event::Warning { source, message } => eprintln!("  ! {source}: {message}"),
            Event::Error { source, message } => eprintln!("  !! {source}: {message}"),
            _ => {}
        }
    }
}

fn feed(engine: &Engine, frame: Frame, wait: bool) {
    if wait {
        let mut tries = 0;
        while !engine.wants_frame(frame.timestamp_ms) && tries < 5000 {
            std::thread::sleep(Duration::from_millis(2));
            tries += 1;
        }
    }
    engine.push_frame(frame);
}

fn run_synthetic(engine: &Engine, fps: f32) {
    for f in synthetic::Scene::three_products().frames(30, fps, 1000, 7) {
        feed(engine, f, true);
    }
}

fn run_images(engine: &Engine, dir: &Path, fps: f32, repeat: u32) -> Result<()> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref(), Some("jpg" | "jpeg" | "png")))
        .collect();
    files.sort();
    if files.is_empty() {
        bail!("no .jpg/.jpeg/.png files in {}", dir.display());
    }
    let step = (1000.0 / fps.max(0.1)) as u64;
    let mut t = 1000u64;
    for p in &files {
        let img = image::open(p).with_context(|| format!("decoding {}", p.display()))?.to_rgb8();
        eprintln!("scanning {}", p.display());
        for _ in 0..repeat.max(1) {
            let frame = Frame { width: img.width(), height: img.height(), format: PixelFormat::Rgb8, data: img.as_raw().clone(), stride: 0, timestamp_ms: t };
            feed(engine, frame, true);
            t += step;
        }
        // a short gap so the tracker closes this photo's tracks before the next view
        t += step * 12;
    }
    Ok(())
}

fn run_ffmpeg(engine: &Engine, a: &Args) -> Result<()> {
    let input = a.input.clone().context("--input is required with --source ffmpeg")?;
    let (w, h) = a.size.split_once('x').and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))).context("--size must look like 640x360")?;
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error"]);
    if let Some(f) = &a.input_format {
        cmd.args(["-f", f]);
    } else {
        // a file would otherwise be decoded far faster than real time, and our timestamps come from the wall clock
        cmd.arg("-re");
    }
    cmd.args(&a.input_args);
    cmd.args(["-i", &input, "-t", &a.seconds.to_string(), "-vf", &format!("scale={w}:{h}"), "-pix_fmt", "rgb24", "-f", "rawvideo", "-"]);
    let mut child = cmd.stdout(Stdio::piped()).spawn().context("could not start ffmpeg (is it installed and on PATH?)")?;
    let mut out = child.stdout.take().context("no ffmpeg stdout")?;
    let size = (w * h * 3) as usize;
    let mut buf = vec![0u8; size];
    let t0 = Instant::now();
    let mut n = 0u64;
    loop {
        match out.read_exact(&mut buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
        let ts = 1 + t0.elapsed().as_millis() as u64;
        n += 1;
        // live source: never block the camera; the engine drops what it cannot take
        if engine.wants_frame(ts) {
            engine.push_frame(Frame { width: w, height: h, format: PixelFormat::Rgb8, data: buf.clone(), stride: 0, timestamp_ms: ts });
        }
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("ffmpeg exited with {status}");
    }
    eprintln!("read {n} frames from ffmpeg");
    Ok(())
}

fn describe(i: &CatalogItem) -> String {
    let state = match &i.state {
        ItemState::Unknown => "no guess yet".to_string(),
        ItemState::Guessed => "guessed".into(),
        ItemState::Confirmed { by: ConfirmedBy::User } => "confirmed".into(),
        ItemState::Confirmed { by: ConfirmedBy::Barcode } => "confirmed by barcode".into(),
        ItemState::Rejected => "rejected".into(),
    };
    let gtin = i.gtin.as_deref().map(|g| format!(" gtin {g}")).unwrap_or_default();
    let title = if i.guess.title.is_empty() { "(no title)" } else { &i.guess.title };
    format!("{}  x{}  {}  [{}{}]", i.id, i.quantity, title, state, gtin)
}

fn prompt(text: &str) -> Option<String> {
    eprint!("{text}");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_string()),
    }
}

async fn review(engine: &Engine) -> Result<()> {
    let queue = engine.review_queue().await?;
    if queue.is_empty() {
        eprintln!("nothing to review");
        return Ok(());
    }
    eprintln!("\nReview {} item(s).  Enter = confirm   r = try another guess   t <title> = correct it   b <digits> = barcode   x = hide   s = skip", queue.len());
    'items: for first in queue {
        let mut item = first;
        loop {
            eprintln!("\n{}", describe(&item));
            let Some(ans) = prompt("> ") else { return Ok(()) };
            let (cmd, rest) = ans.split_once(' ').map(|(c, r)| (c, r.trim())).unwrap_or((ans.as_str(), ""));
            let res = match cmd {
                "" => {
                    engine.confirm_item(item.id.clone()).await?;
                    continue 'items;
                }
                "s" => continue 'items,
                "x" => {
                    engine.set_role(item.id.clone(), Role::Excluded, true).await?;
                    continue 'items;
                }
                "r" => engine.reject_item(item.id.clone(), None).await,
                "t" if !rest.is_empty() => engine.reject_item(item.id.clone(), Some(rest.to_string())).await,
                "b" if !rest.is_empty() => engine.submit_barcode(item.id.clone(), rest.to_string()).await,
                _ => {
                    eprintln!("?  (Enter, r, t <title>, b <digits>, x, s)");
                    continue;
                }
            };
            match res {
                Ok(updated) => {
                    engine.flush().await?; // waits for a re-guess or lookup that this started
                    item = engine.item(updated.id.clone()).await?.unwrap_or(updated);
                    if matches!(item.state, ItemState::Confirmed { .. }) {
                        eprintln!("{}", describe(&item));
                        continue 'items; // a typed title or a valid barcode already settles the item
                    }
                }
                Err(e) => eprintln!("  {e}"),
            }
        }
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    if let Some(Cmd::Fixtures { out }) = &args.command {
        return fixtures::write(out);
    }
    std::fs::create_dir_all(args.out.join("crops"))?;

    let cfg = EngineConfig {
        analysis_fps: args.fps,
        default_mode: match args.mode {
            ModeArg::Barcode => ScanMode::BarcodeFirst,
            ModeArg::Describe => ScanMode::DescribeFirst,
        },
        sku_prefix: args.sku_prefix.clone(),
        // offline sources are replayed, so repeated frames are expected; live capture keeps the default gates
        novelty_threshold: if matches!(args.source, Source::Ffmpeg) { EngineConfig::default().novelty_threshold } else { 0.0 },
        blur_gate_ratio: if matches!(args.source, Source::Ffmpeg) { EngineConfig::default().blur_gate_ratio } else { 0.0 },
        ..EngineConfig::default()
    };
    let mut builder = EngineBuilder::new(cfg).listener(Arc::new(Printer));
    if args.anthropic {
        let key = std::env::var("ANTHROPIC_API_KEY").context("--anthropic needs ANTHROPIC_API_KEY in the environment")?;
        eprintln!("cloud: masked product crops will be sent to the Anthropic API");
        builder = builder.anthropic(key, args.model.clone());
    }
    if let Some(c) = &args.off_contact {
        builder = builder.open_food_facts(c.clone());
    }
    if let Some(p) = &args.sqlite {
        builder = builder.sqlite_store(p.to_string_lossy().into_owned())?;
    }
    let engine = builder.build()?;

    eprintln!("scanning...");
    match args.source {
        Source::Synthetic => run_synthetic(&engine, args.fps),
        Source::Images => run_images(&engine, Path::new(args.input.as_deref().context("--input <folder> is required with --source images")?), args.fps, args.repeat)?,
        Source::Ffmpeg => run_ffmpeg(&engine, &args)?,
    }
    let summary = engine.flush().await?;
    eprintln!("\n{} frame(s) analysed, {} product(s) found", summary.frames_processed, summary.items);

    let items = engine.catalog(false).await?;
    for (n, i) in items.iter().enumerate() {
        if let Some(jpeg) = i.images.first() {
            std::fs::write(args.out.join("crops").join(format!("{}.jpg", i.id)), jpeg)?;
        }
        eprintln!("{:>3}. {}", n + 1, describe(i));
    }

    if args.review {
        review(&engine).await?;
    }

    // Without --review nothing was confirmed: the drafts say so and keep "needs your input" markers.
    let listings = engine.generate_listings(!args.review).await?;
    if listings.is_empty() {
        eprintln!("no listings to draft");
    }
    let ids: Vec<String> = listings.iter().map(|l| l.id.clone()).collect();

    let csv_path = args.out.join("products.csv");
    let csv = storefront::CsvStorefront::new(csv_path.to_string_lossy().into_owned(), false);
    let receipts = engine.publish(ids.clone(), csv).await?;
    let ok = receipts.iter().filter(|r| r.ok).count();
    eprintln!("\n{ok}/{} draft(s) written to {}", receipts.len(), csv_path.display());
    for r in receipts.iter().filter(|r| !r.ok) {
        eprintln!("  {} failed: {}", r.listing_id, r.message);
    }

    if let Some(shop) = &args.shopify_shop {
        let sf = match (std::env::var("SHOPIFY_ADMIN_TOKEN"), std::env::var("SHOPIFY_CLIENT_ID"), std::env::var("SHOPIFY_CLIENT_SECRET")) {
            (Ok(token), _, _) => storefront::ShopifyStorefront::new(shop.clone(), token, None, None)?,
            (_, Ok(id), Ok(secret)) => storefront::ShopifyStorefront::with_client_credentials(shop.clone(), id, secret, None, None)?,
            _ => bail!("--shopify-shop needs SHOPIFY_ADMIN_TOKEN, or SHOPIFY_CLIENT_ID and SHOPIFY_CLIENT_SECRET, in the environment"),
        };
        let receipts = engine.publish(ids, sf).await?;
        for r in &receipts {
            if r.ok {
                eprintln!("  shopify draft: {}", r.url.as_deref().or(r.remote_id.as_deref()).unwrap_or("created"));
            } else {
                eprintln!("  shopify failed ({}): {}", r.listing_id, r.message);
            }
        }
    }

    std::fs::write(args.out.join("catalog.json"), serde_json::to_string_pretty(&engine.catalog(true).await?)?)?;
    eprintln!("catalog.json and crops/ are in {}", args.out.display());
    engine.shutdown();
    Ok(())
}
