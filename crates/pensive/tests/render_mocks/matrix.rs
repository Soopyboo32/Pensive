//! `cargo mocks --matrix FILTER...` renders window mocks at every window size, zoom and density
//! in [`mock::matrix`], to `target/mocks-matrix/<subject>/<mock>/<cell>.png`, where a cell is
//! named like `960x1080-zoom200-calm`. It renders each one whose `<subject>/<mock>/<cell>`
//! contains every filter, and needs at least one filter, since every window mock in every cell is
//! more images than anyone looks at.

use std::{path::Path, process::ExitCode};

use mock::matrix::Cell;

use crate::{Args, Line, Writer, catching_panics, print, save};

pub(crate) fn run(args: &Args, folder: &Path) -> gpui::Result<ExitCode> {
    if args.filters.is_empty() {
        eprintln!("name the mocks to render, as in `cargo mocks --matrix settings/appearance`");
        return Ok(ExitCode::FAILURE);
    }
    let writer = Writer::new(args.threads);
    let mut rendered = 0;
    let mut failed = Vec::new();
    for mock in ui::every_mock().iter().filter(|mock| mock.in_matrix) {
        let mut lines = Vec::new();
        for cell in Cell::all() {
            let id = format!("{}/{}", mock.id(), cell.name());
            let matches = args
                .filters
                .iter()
                .all(|filter| id.contains(filter.as_str()));
            if !matches {
                continue;
            }
            let image = match catching_panics(|| mock.render_in(&cell)) {
                Ok(Some(image)) => image,
                Ok(None) => {
                    println!("skipped: this platform has no headless renderer");
                    return Ok(ExitCode::SUCCESS);
                }
                Err(error) => {
                    lines.push(Line::Error(format!("FAILED {id}: {error}")));
                    failed.push(id);
                    continue;
                }
            };
            rendered += 1;
            let path = folder.join(format!("{id}.png"));
            let saving = writer.run({
                let path = path.clone();
                move || save(&image, &path).map(|()| None)
            });
            lines.push(Line::Saved { path, saving });
        }
        print(lines)?;
    }

    if rendered == 0 && failed.is_empty() {
        eprintln!("no window mock matches `{}`", args.filters.join(" "));
        return Ok(ExitCode::FAILURE);
    }
    if failed.is_empty() {
        println!("{rendered} renders");
        return Ok(ExitCode::SUCCESS);
    }
    eprintln!(
        "{rendered} renders, {} failed: {}",
        failed.len(),
        failed.join(", ")
    );
    Ok(ExitCode::FAILURE)
}
