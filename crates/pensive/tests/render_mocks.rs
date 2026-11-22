//! Renders every mock to `target/mocks/<subject>/<mock>.png`, and each subject with more than one
//! mock to an atlas at `target/mocks/<subject>.png`, continued in `<subject>-2.png` and on when
//! it's too large for one image. A filter renders only the mocks whose `<subject>/<mock>` contains
//! it. The atlas is only written when the filter matches every mock of its subject, and an old one
//! is deleted when it matches only some.
//!
//! `cargo mocks <filter>` passes the filter with `--only`, and fails when nothing matches it.
//! `cargo test <filter>` passes it bare to every test binary, so here, as in any other test
//! binary, a filter that matches nothing renders nothing and passes.
//!
//! A mock that fails or panics is reported by id, and the rest still render.
//!
//! Subjects render on several threads at once and PNGs save on others, but each subject's lines
//! print together and in order.
//!
//! `--save-baseline` also copies each render to `target/mocks-baseline`. While that folder
//! exists, every later render is compared with its copy there: each one that changed is listed,
//! with a picture of what changed in `target/mocks-diff`. A change is reported, never a failure,
//! since changing how a view looks is the point of most UI work.

use std::{
    any::Any,
    cmp::Reverse,
    io::ErrorKind,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
};

use anyhow::anyhow;
use image::RgbaImage;
use mock::{Atlas, Mock, diff::diff};

/// libtest's options that take a value, so a value such as the `1` in `--test-threads 1` isn't
/// read as a filter.
const VALUE_OPTIONS: &[&str] = &[
    "--test-threads",
    "--skip",
    "--format",
    "--color",
    "--logfile",
    "--shuffle-seed",
    "-Z",
];

struct Args {
    filter: String,
    /// Whether matching nothing is a failure, as it is for `cargo mocks`.
    strict: bool,
    save_baseline: bool,
    /// How many renders, and separately how many saves, run at once. `--test-threads 1` renders
    /// one mock at a time, as libtest runs one test at a time.
    threads: usize,
}

impl Args {
    fn parse(mut args: impl Iterator<Item = String>) -> Self {
        let mut parsed = Self {
            filter: String::new(),
            strict: false,
            save_baseline: false,
            threads: thread::available_parallelism().map_or(4, |count| count.get()),
        };
        let mut filter = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--only" => parsed.strict = true,
                "--save-baseline" => parsed.save_baseline = true,
                "--test-threads" => {
                    if let Some(threads) = args.next().and_then(|threads| threads.parse().ok()) {
                        parsed.threads = usize::max(threads, 1);
                    }
                }
                option if VALUE_OPTIONS.contains(&option) => {
                    args.next();
                }
                _ if !arg.starts_with('-') && filter.is_none() => filter = Some(arg),
                _ => {}
            }
        }
        parsed.filter = filter.unwrap_or_default();
        parsed
    }
}

/// Where renders go, and the copies they're compared with.
struct Folders {
    mocks: PathBuf,
    baseline: PathBuf,
    diff: PathBuf,
}

impl Folders {
    fn new() -> Self {
        let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .parent()
            .expect("the target tmp dir is inside the target dir");
        Self {
            mocks: target.join("mocks"),
            baseline: target.join("mocks-baseline"),
            diff: target.join("mocks-diff"),
        }
    }
}

/// How one render compares with its baseline.
enum Comparison {
    Same,
    Changed(String),
    New,
}

/// What happens to each render besides saving it.
#[derive(Clone, Copy)]
enum Keep {
    Save,
    SaveBaseline,
    Compare,
}

fn main() -> gpui::Result<ExitCode> {
    let args = Args::parse(std::env::args().skip(1));
    let folders = Arc::new(Folders::new());
    let keep = if args.save_baseline {
        Keep::SaveBaseline
    } else if folders.baseline.is_dir() {
        Keep::Compare
    } else {
        Keep::Save
    };

    let mocks = ui::every_mock();
    let subjects = matching_subjects(&mocks, &args.filter);
    let writer = Writer::new(folders.clone(), args.threads);
    let (senders, receivers): (Vec<_>, Vec<_>) = subjects.iter().map(|_| mpsc::channel()).unzip();
    // The largest subjects start first, so the last one left isn't a large one that started late.
    let mut queue: Vec<_> = subjects.iter().zip(senders).collect();
    queue.sort_by_key(|(subject, _)| Reverse(subject.mocks.len()));
    let queue = Mutex::new(queue.into_iter());

    let mut rendered = 0;
    let mut failed = Vec::new();
    let headless = thread::scope(|scope| -> gpui::Result<bool> {
        for _ in 0..args.threads {
            scope.spawn(|| {
                loop {
                    let next = queue
                        .lock()
                        .expect("no thread panics holding the lock")
                        .next();
                    let Some((subject, report)) = next else {
                        return;
                    };
                    let result =
                        catching_panics(|| render_subject(subject, &folders, &writer, keep));
                    // Sending fails only once `main` has stopped reading on an error.
                    report.send(result).ok();
                }
            });
        }
        // Each subject's lines print together, in the order of `every_mock`.
        for receiver in receivers {
            let report = receiver.recv()??;
            if report.no_renderer {
                return Ok(false);
            }
            for line in report.lines {
                match line {
                    Line::Out(line) => println!("{line}"),
                    Line::Err(line) => eprintln!("{line}"),
                }
            }
            rendered += report.rendered;
            failed.extend(report.failed);
        }
        Ok(true)
    })?;
    if !headless {
        println!("skipped: this platform has no headless renderer");
        return Ok(ExitCode::SUCCESS);
    }

    let changes = writer.finish()?;
    for change in &changes {
        println!("{change}");
    }

    if rendered == 0 && failed.is_empty() {
        eprintln!("no mock matches `{}`", args.filter);
        return Ok(if args.strict {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        });
    }
    let mut summary = format!("{rendered} mocks rendered");
    match keep {
        Keep::Save => {}
        Keep::SaveBaseline => {
            summary += &format!(", saved as the baseline in {}", folders.baseline.display());
        }
        Keep::Compare => {
            summary += &format!(", {} changed since the baseline", changes.len());
        }
    }
    if failed.is_empty() {
        println!("{summary}");
        return Ok(ExitCode::SUCCESS);
    }
    eprintln!("{summary}, {} failed: {}", failed.len(), failed.join(", "));
    Ok(ExitCode::FAILURE)
}

/// The mocks of one subject that the filter matches.
struct Subject<'a> {
    name: &'static str,
    /// Each with its place among every matching mock, which orders the changes `main` lists.
    mocks: Vec<(usize, &'a Mock)>,
    /// Whether the subject has more than one mock and the filter matches all of them.
    atlas: bool,
}

/// Each subject with a mock whose id contains `filter`, in the order of `mocks`.
fn matching_subjects<'a>(mocks: &'a [Mock], filter: &str) -> Vec<Subject<'a>> {
    let mut index = 0;
    mocks
        .chunk_by(|a, b| a.subject == b.subject)
        .filter_map(|subject_mocks| {
            let matching: Vec<(usize, &Mock)> = subject_mocks
                .iter()
                .filter(|mock| mock.id().contains(filter))
                .map(|mock| {
                    index += 1;
                    (index - 1, mock)
                })
                .collect();
            Some(Subject {
                name: matching.first()?.1.subject,
                atlas: subject_mocks.len() > 1 && matching.len() == subject_mocks.len(),
                mocks: matching,
            })
        })
        .collect()
}

/// What rendering one subject printed and found.
#[derive(Default)]
struct Report {
    lines: Vec<Line>,
    rendered: usize,
    failed: Vec<String>,
    /// This platform has no headless renderer, so nothing rendered.
    no_renderer: bool,
}

/// A line for stdout or stderr, held until the subjects before it have printed theirs.
enum Line {
    Out(String),
    Err(String),
}

/// Renders a subject's mocks, then its atlas, and hands each image to `writer` to save.
fn render_subject(
    subject: &Subject,
    folders: &Folders,
    writer: &Writer,
    keep: Keep,
) -> gpui::Result<Report> {
    let mut report = Report::default();
    let mut atlas = subject.atlas.then(|| Atlas::new(subject.name));
    for &(index, mock) in &subject.mocks {
        let image = match catching_panics(|| mock.render()) {
            Ok(Some(image)) => image,
            Ok(None) => {
                report.no_renderer = true;
                return Ok(report);
            }
            Err(error) => {
                report
                    .lines
                    .push(Line::Err(format!("FAILED {}: {error}", mock.id())));
                report.failed.push(mock.id());
                atlas = None;
                continue;
            }
        };
        let path = folders.mocks.join(format!("{}.png", mock.id()));
        report.lines.push(Line::Out(path.display().to_string()));
        report.rendered += 1;
        if let Some(atlas) = &mut atlas {
            atlas.push(mock, image.clone());
        }
        writer.mock(index, mock.id(), image, keep);
    }

    let pages = match atlas {
        Some(atlas) => atlas.render()?.expect("the mocks above rendered headless"),
        None => Vec::new(),
    };
    let page_count = pages.len();
    for (index, page) in pages.into_iter().enumerate() {
        let path = atlas_page(folders, subject.name, index);
        report.lines.push(Line::Out(path.display().to_string()));
        writer.save(page, path);
    }
    remove_atlas_pages(folders, subject.name, page_count)?;
    Ok(report)
}

/// Saves and compares renders on other threads while the next ones draw, since encoding a PNG
/// takes longer than drawing it.
struct Writer {
    jobs: SyncSender<Job>,
    workers: Vec<JoinHandle<gpui::Result<Vec<Change>>>>,
}

enum Job {
    Mock {
        index: usize,
        id: String,
        image: RgbaImage,
        keep: Keep,
    },
    Page {
        image: RgbaImage,
        path: PathBuf,
    },
}

/// A line reporting a render that differs from its baseline, with the index of its mock so the
/// lines print in the order the mocks rendered.
struct Change {
    index: usize,
    line: String,
}

impl Writer {
    fn new(folders: Arc<Folders>, threads: usize) -> Self {
        // No queue, so a render waits for a free worker and only as many renders as there are
        // workers wait in memory.
        let (jobs, receiver) = mpsc::sync_channel(0);
        let receiver = Arc::new(Mutex::new(receiver));
        let workers = (0..threads)
            .map(|_| {
                let receiver = receiver.clone();
                let folders = folders.clone();
                thread::spawn(move || work(&receiver, &folders))
            })
            .collect();
        Self { jobs, workers }
    }

    /// Saves a mock's render, and saves it to or compares it with the baseline as `keep` says.
    fn mock(&self, index: usize, id: String, image: RgbaImage, keep: Keep) {
        self.send(Job::Mock {
            index,
            id,
            image,
            keep,
        });
    }

    fn save(&self, image: RgbaImage, path: PathBuf) {
        self.send(Job::Page { image, path });
    }

    /// Sending fails only once every worker has stopped on an error, which `finish` returns.
    fn send(&self, job: Job) {
        self.jobs.send(job).ok();
    }

    /// Waits for every job, and returns the changes since the baseline in the order the mocks
    /// rendered, or the first error.
    fn finish(self) -> gpui::Result<Vec<String>> {
        drop(self.jobs);
        let mut changes = Vec::new();
        for worker in self.workers {
            let worker = worker
                .join()
                .unwrap_or_else(|panic| Err(anyhow!("panicked: {}", panic_message(&*panic))));
            changes.extend(worker?);
        }
        changes.sort_by_key(|change| change.index);
        Ok(changes.into_iter().map(|change| change.line).collect())
    }
}

/// Runs jobs until the writer finishes or one fails.
fn work(jobs: &Mutex<Receiver<Job>>, folders: &Folders) -> gpui::Result<Vec<Change>> {
    let mut changes = Vec::new();
    loop {
        let job = jobs
            .lock()
            .expect("no worker panics holding the lock")
            .recv();
        match job {
            Err(_) => return Ok(changes),
            Ok(Job::Page { image, path }) => save(&image, &path)?,
            Ok(Job::Mock {
                index,
                id,
                image,
                keep,
            }) => {
                let file = format!("{id}.png");
                save(&image, &folders.mocks.join(&file))?;
                let line = match keep {
                    Keep::Save => continue,
                    Keep::SaveBaseline => {
                        save(&image, &folders.baseline.join(&file))?;
                        continue;
                    }
                    Keep::Compare => match compare(&image, &file, folders)? {
                        Comparison::Same => continue,
                        Comparison::Changed(summary) => format!("changed {id}: {summary}"),
                        Comparison::New => format!("new {id}: no baseline to compare with"),
                    },
                };
                changes.push(Change { index, line });
            }
        }
    }
}

/// Compares the render saved as `file` with its baseline, and writes a picture of what changed to
/// the diff folder, or removes an old one.
fn compare(image: &RgbaImage, file: &str, folders: &Folders) -> gpui::Result<Comparison> {
    let diff_path = folders.diff.join(file);
    let baseline = match image::open(folders.baseline.join(file)) {
        Ok(baseline) => baseline.into_rgba8(),
        Err(image::ImageError::IoError(error)) if error.kind() == ErrorKind::NotFound => {
            remove(&diff_path)?;
            return Ok(Comparison::New);
        }
        Err(error) => return Err(error.into()),
    };
    let Some(diff) = diff(&baseline, image) else {
        remove(&diff_path)?;
        return Ok(Comparison::Same);
    };
    save(&diff.image, &diff_path)?;
    let (x, y, width, height) = diff.bounds;
    Ok(Comparison::Changed(format!(
        "{} px in {width}×{height} at ({x}, {y}), see {}",
        diff.changed,
        diff_path.display()
    )))
}

/// `<subject>.png` for the first page of an atlas, then `<subject>-2.png` and on.
fn atlas_page(folders: &Folders, subject: &str, index: usize) -> PathBuf {
    let file = match index {
        0 => format!("{subject}.png"),
        index => format!("{subject}-{}.png", index + 1),
    };
    folders.mocks.join(file)
}

/// Removes the pages from `from` on, left by an earlier render with more of them.
fn remove_atlas_pages(folders: &Folders, subject: &str, from: usize) -> gpui::Result<()> {
    for index in from.. {
        let path = atlas_page(folders, subject, index);
        if !path.exists() {
            return Ok(());
        }
        remove(&path)?;
    }
    Ok(())
}

fn save(image: &RgbaImage, path: &Path) -> gpui::Result<()> {
    std::fs::create_dir_all(path.parent().expect("the path has a folder"))?;
    image.save(path)?;
    Ok(())
}

fn remove(path: &Path) -> gpui::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

/// Runs `f`, turning a panic into an error so the mocks after it still render.
fn catching_panics<T>(f: impl FnOnce() -> gpui::Result<T>) -> gpui::Result<T> {
    catch_unwind(AssertUnwindSafe(f))
        .unwrap_or_else(|panic| Err(anyhow!("panicked: {}", panic_message(&*panic))))
}

fn panic_message(panic: &(dyn Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("no message")
}
