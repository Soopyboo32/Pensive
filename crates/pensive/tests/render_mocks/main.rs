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
//! Subjects render on several threads at once, except on macOS, and PNGs save on others. Each
//! path prints once its PNG is saved, in the order of `every_mock`.
//!
//! `--save-baseline` also copies each render to `target/mocks-baseline`. While that folder
//! exists, every later render is compared with its copy there: each one that changed is listed,
//! with a picture of what changed in `target/mocks-diff`. A change is reported, never a failure,
//! since changing how a view looks is the point of most UI work.
//!
//! `--matrix` renders window mocks across window sizes instead. See [`matrix`].

mod matrix;

use std::{
    any::Any,
    cmp::Reverse,
    io::ErrorKind,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
};

use anyhow::{Context as _, anyhow};
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
    /// The words on the command line that aren't options. Only the first filters a normal run.
    filters: Vec<String>,
    /// Whether matching nothing is a failure, as it is for `cargo mocks`.
    strict: bool,
    save_baseline: bool,
    matrix: bool,
    /// How many saves run at once, and how many renders up to `MAX_RENDER_THREADS`. Taken from
    /// `--test-threads` or `RUST_TEST_THREADS`, as libtest does. With 1, mocks render one at a time
    /// on the main thread.
    threads: usize,
}

impl Args {
    fn parse(mut args: impl Iterator<Item = String>) -> Self {
        let mut parsed = Self {
            filters: Vec::new(),
            strict: false,
            save_baseline: false,
            matrix: false,
            threads: std::env::var("RUST_TEST_THREADS")
                .ok()
                .and_then(|threads| threads.parse().ok())
                .unwrap_or_else(|| thread::available_parallelism().map_or(4, |count| count.get()))
                .max(1),
        };
        while let Some(arg) = args.next() {
            // An option's value follows it, or an `=` joins them.
            let (option, joined) = match arg.split_once('=') {
                Some((option, value)) if option.starts_with("--") => (option, Some(value)),
                _ => (arg.as_str(), None),
            };
            match option {
                "--only" => parsed.strict = true,
                "--save-baseline" => parsed.save_baseline = true,
                "--matrix" => parsed.matrix = true,
                "--test-threads" => {
                    let value = joined.map(str::to_owned).or_else(|| args.next());
                    if let Some(threads) = value.and_then(|threads| threads.parse::<usize>().ok()) {
                        parsed.threads = threads.max(1);
                    }
                }
                option if VALUE_OPTIONS.contains(&option) => {
                    if joined.is_none() {
                        args.next();
                    }
                }
                _ if !arg.starts_with('-') => parsed.filters.push(arg),
                _ => {}
            }
        }
        parsed
    }

    fn filter(&self) -> &str {
        self.filters.first().map_or("", String::as_str)
    }
}

/// Where renders go, and the copies they're compared with.
struct Folders {
    mocks: PathBuf,
    baseline: PathBuf,
    diff: PathBuf,
    matrix: PathBuf,
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
            matrix: target.join("mocks-matrix"),
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

/// The most mocks that render at once. More threads barely help, and each holds a subject's
/// renders until its atlas is drawn.
const MAX_RENDER_THREADS: usize = 8;

fn main() -> gpui::Result<ExitCode> {
    let args = Args::parse(std::env::args().skip(1));
    let folders = Arc::new(Folders::new());
    if args.matrix {
        return matrix::run(&args, &folders.matrix);
    }
    let keep = if args.save_baseline {
        Keep::SaveBaseline
    } else if folders.baseline.is_dir() {
        Keep::Compare
    } else {
        Keep::Save
    };
    // macOS builds its platform, which the text system comes from, only on the main thread.
    let render_threads = if cfg!(target_os = "macos") {
        1
    } else {
        args.threads.min(MAX_RENDER_THREADS)
    };

    let mocks = ui::every_mock();
    let subjects = matching_subjects(&mocks, args.filter());
    let writer = Writer::new(args.threads);
    let mut headless = true;
    let mut rendered = 0;
    let mut changed = 0;
    let mut failed = Vec::new();
    render_in_order(
        &subjects,
        render_threads,
        |subject| catching_panics(|| render_subject(subject, &folders, &writer, keep)),
        |report| {
            if report.no_renderer {
                headless = false;
                return Ok(false);
            }
            changed += print(report.lines)?;
            rendered += report.rendered;
            failed.extend(report.failed);
            Ok(true)
        },
    )?;
    if !headless {
        println!("skipped: this platform has no headless renderer");
        return Ok(ExitCode::SUCCESS);
    }

    if rendered == 0 && failed.is_empty() {
        eprintln!("no mock matches `{}`", args.filter());
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
        Keep::Compare => summary += &format!(", {changed} changed since the baseline"),
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
    mocks: Vec<&'a Mock>,
    /// Whether the subject has more than one mock and the filter matches all of them.
    atlas: bool,
}

/// Each subject with a mock whose id contains `filter`, in the order of `mocks`.
fn matching_subjects<'a>(mocks: &'a [Mock], filter: &str) -> Vec<Subject<'a>> {
    mocks
        .chunk_by(|a, b| a.subject == b.subject)
        .filter_map(|subject_mocks| {
            let matching: Vec<&Mock> = subject_mocks
                .iter()
                .filter(|mock| mock.id().contains(filter))
                .collect();
            Some(Subject {
                name: matching.first()?.subject,
                atlas: subject_mocks.len() > 1 && matching.len() == subject_mocks.len(),
                mocks: matching,
            })
        })
        .collect()
}

/// Renders each subject with `render` on `threads` threads, or on this one when `threads` is 1,
/// and hands the reports to `each` in the order of `subjects` until it returns false.
fn render_in_order(
    subjects: &[Subject],
    threads: usize,
    render: impl Fn(&Subject) -> gpui::Result<Report> + Sync,
    mut each: impl FnMut(Report) -> gpui::Result<bool>,
) -> gpui::Result<()> {
    if threads <= 1 {
        for subject in subjects {
            if !each(render(subject)?)? {
                break;
            }
        }
        return Ok(());
    }

    let (senders, receivers): (Vec<_>, Vec<_>) = subjects.iter().map(|_| mpsc::channel()).unzip();
    // The largest subjects start first, so the last one left isn't a large one that started late.
    let mut queue: Vec<_> = subjects.iter().zip(senders).collect();
    queue.sort_by_key(|(subject, _)| Reverse(subject.mocks.len()));
    let queue = Mutex::new(queue.into_iter());
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        for _ in 0..threads.min(subjects.len()) {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let next = queue
                        .lock()
                        .expect("no thread panics holding the lock")
                        .next();
                    let Some((subject, report)) = next else {
                        return;
                    };
                    // Sending fails only once the reports stopped being read.
                    report.send(render(subject)).ok();
                }
            });
        }
        let result = read_in_order(receivers, &mut each);
        // The threads finish the subjects they're on, and start no more.
        stop.store(true, Ordering::Relaxed);
        result
    })
}

/// Hands each report to `each` as soon as it and the ones before it are in.
fn read_in_order(
    receivers: Vec<Receiver<gpui::Result<Report>>>,
    each: &mut impl FnMut(Report) -> gpui::Result<bool>,
) -> gpui::Result<()> {
    for receiver in receivers {
        if !each(receiver.recv()??)? {
            break;
        }
    }
    Ok(())
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

/// A line to print once the subjects before it have printed theirs.
enum Line {
    Error(String),
    /// A PNG's path, printed once it's saved, then how it changed from its baseline if it did.
    Saved {
        path: PathBuf,
        saving: Receiver<gpui::Result<Option<String>>>,
    },
}

/// Prints `lines`, waiting for each PNG to be saved before printing its path, and returns how
/// many renders changed from their baseline.
fn print(lines: Vec<Line>) -> gpui::Result<usize> {
    let mut changed = 0;
    for line in lines {
        match line {
            Line::Error(line) => eprintln!("{line}"),
            Line::Saved { path, saving } => {
                let change = saving
                    .recv()
                    .map_err(|_| anyhow!("panicked"))
                    .and_then(|saved| saved)
                    .with_context(|| format!("couldn't save {}", path.display()))?;
                println!("{}", path.display());
                if let Some(change) = change {
                    changed += 1;
                    println!("{change}");
                }
            }
        }
    }
    Ok(changed)
}

/// Renders a subject's mocks, then its atlas, and hands each image to `writer` to save.
fn render_subject(
    subject: &Subject,
    folders: &Arc<Folders>,
    writer: &Writer,
    keep: Keep,
) -> gpui::Result<Report> {
    let mut report = Report::default();
    let mut atlas = subject.atlas.then(|| Atlas::new(subject.name));
    for &mock in &subject.mocks {
        let image = match catching_panics(|| mock.render()) {
            Ok(Some(image)) => image,
            Ok(None) => {
                report.no_renderer = true;
                return Ok(report);
            }
            Err(error) => {
                report
                    .lines
                    .push(Line::Error(format!("FAILED {}: {error}", mock.id())));
                report.failed.push(mock.id());
                atlas = None;
                continue;
            }
        };
        report.rendered += 1;
        if let Some(atlas) = &mut atlas {
            atlas.push(mock, image.clone());
        }
        let id = mock.id();
        let path = folders.mocks.join(format!("{id}.png"));
        let folders = folders.clone();
        let saving = writer.run(move || save_mock(&image, &id, keep, &folders));
        report.lines.push(Line::Saved { path, saving });
    }

    let pages = match atlas {
        Some(atlas) => atlas.render()?.expect("the mocks above rendered headless"),
        None => Vec::new(),
    };
    let page_count = pages.len();
    for (index, page) in pages.into_iter().enumerate() {
        let path = atlas_page(folders, subject.name, index);
        let saving = writer.run({
            let path = path.clone();
            move || save(&page, &path).map(|()| None)
        });
        report.lines.push(Line::Saved { path, saving });
    }
    remove_atlas_pages(folders, subject.name, page_count)?;
    Ok(report)
}

/// Saves a mock's render, and saves it to or compares it with the baseline as `keep` says.
/// Returns the line reporting how it changed from its baseline, if it did.
fn save_mock(
    image: &RgbaImage,
    id: &str,
    keep: Keep,
    folders: &Folders,
) -> gpui::Result<Option<String>> {
    let file = format!("{id}.png");
    save(image, &folders.mocks.join(&file))?;
    Ok(match keep {
        Keep::Save => None,
        Keep::SaveBaseline => {
            save(image, &folders.baseline.join(&file))?;
            None
        }
        Keep::Compare => match compare(image, &file, folders)? {
            Comparison::Same => None,
            Comparison::Changed(summary) => Some(format!("changed {id}: {summary}")),
            Comparison::New => Some(format!("new {id}: no baseline to compare with")),
        },
    })
}

type Job = Box<dyn FnOnce() + Send>;

/// Saves and compares renders on other threads while the next ones draw, since encoding a PNG
/// takes longer than drawing it. Dropping it waits for every job, so no PNG is left half written.
struct Writer {
    jobs: Option<SyncSender<Job>>,
    workers: Vec<JoinHandle<()>>,
}

impl Writer {
    fn new(threads: usize) -> Self {
        // No queue, so a render waits for a free worker and only as many renders as there are
        // workers wait in memory.
        let (jobs, receiver) = mpsc::sync_channel::<Job>(0);
        let receiver = Arc::new(Mutex::new(receiver));
        let workers = (0..threads)
            .map(|_| {
                let receiver = receiver.clone();
                thread::spawn(move || {
                    loop {
                        let job = receiver
                            .lock()
                            .expect("no worker panics holding the lock")
                            .recv();
                        let Ok(job) = job else {
                            return;
                        };
                        // A job that panics drops its sender, which its receiver reports.
                        catch_unwind(AssertUnwindSafe(job)).ok();
                    }
                })
            })
            .collect();
        Self {
            jobs: Some(jobs),
            workers,
        }
    }

    /// Runs `job` on a worker, and returns where its result arrives.
    fn run<T: Send + 'static>(&self, job: impl FnOnce() -> T + Send + 'static) -> Receiver<T> {
        let (done, result) = mpsc::channel();
        let job: Job = Box::new(move || {
            done.send(job()).ok();
        });
        self.jobs
            .as_ref()
            .expect("the writer is running")
            .send(job)
            .expect("workers run until the writer drops");
        result
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        drop(self.jobs.take());
        for worker in self.workers.drain(..) {
            worker.join().ok();
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
