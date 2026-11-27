---
name: responsiveness
description: Render a window view at many window sizes, zooms and densities with `cargo mocks --matrix`, and look for layouts that break.
disable-model-invocation: true
---

# Responsiveness

Renders window mocks at 9 window sizes, at 80% and 200% zoom, in both densities, then looks at the images for layouts that break. Nothing checks the images for you: looking at them is the check.

The invocation ($ARGUMENTS in Claude Code) names the views to check, as mock ids or parts of them, such as `settings/appearance` or `project`. Without one, use the window mocks of the views the current diff changes. If the diff touches no view, ask which to check.

## Rendering

1. Run `cargo mocks --matrix <filter>` for each view. A filter matches part of `<subject>/<mock>/<cell>`, and every filter given must match, so `cargo mocks --matrix settings/appearance zoom200` renders one mock at 200% only. A view renders 36 images in about 10 seconds.
2. Each image is at `target/mocks-matrix/<subject>/<mock>/<cell>.png`, with cells named like `960x1080-zoom200-calm`. The PNGs are at twice the window's size.
3. If no window mock matches, the view has no mock made with `mock_window::mock`. Mocks made with `mock_window::fixed` set their own zoom or density, so the matrix leaves them out. Say so, and don't add a mock unless asked.
4. Also run `cargo mocks <subject>/<mock>` and look at `target/mocks/<subject>/<mock>.png`. That's the view at 1920x1080 and the default zoom and density, as it's meant to look.

To tell a new break from one that was already there, render the matrix before changing the layout and move it aside with `mv target/mocks-matrix target/mocks-matrix-before`.

## Looking

Look at the cramped cells first. Zoom divides the room a window has, so at 200% the windows act half their size:

1. `720x480-zoom200-calm` and `-dense`: the smallest window Pensive allows, at the largest zoom.
2. `960x540-zoom200-calm`, `960x1080-zoom200-calm` and `1080x1920-zoom200-calm`: narrow and portrait.
3. `3440x1440-zoom80-calm`: the widest.

If those look right, the cells in between usually do too. Look at the rest of the `zoom200` cells when a cramped one breaks, to find the size it starts at.

In each image, look for:

- Text that wraps after every letter or every word, so it runs down in a column.
- Side columns that leave the main area too narrow to use, or push it out of the window.
- Text or controls cut off at an edge with no fade or ellipsis, or past the window's edge.
- Content drawn over other content.
- The landing screen or the palette off center.
- In wide windows, content stretched too wide to read, or stranded in empty space.

## Reporting

For each problem, give the PNG's path, what's wrong, and the smallest cell it shows in. Say which cells you looked at and found fine. If you rendered a before folder, say which problems are new. Fix a problem only if the task asked for it.
