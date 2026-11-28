//! The window sizes, zooms and densities `cargo mocks --matrix` renders window mocks at.

use theme::{DensityPreset, VariantArray};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}

const fn size(width: u32, height: u32) -> WindowSize {
    WindowSize { width, height }
}

pub const SIZES: [WindowSize; 9] = [
    // Half and a quarter of a 1080p screen, and one on its side.
    size(960, 1080),
    size(960, 540),
    size(1080, 1920),
    // Half of a 1440p screen.
    size(1280, 1440),
    // An ultrawide screen, and half of one.
    size(3440, 1440),
    size(1720, 1440),
    // Laptops.
    size(1280, 800),
    size(1440, 900),
    // The smallest window Pensive allows.
    size(720, 480),
];

pub const ZOOMS: [u32; 2] = [80, 200];

/// One window size at one zoom and density.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub size: WindowSize,
    pub zoom: u32,
    pub density: DensityPreset,
}

impl Cell {
    /// Every window size at every zoom and density.
    pub fn all() -> impl Iterator<Item = Self> {
        SIZES.into_iter().flat_map(|size| {
            ZOOMS.into_iter().flat_map(move |zoom| {
                DensityPreset::VARIANTS.iter().map(move |&density| Self {
                    size,
                    zoom,
                    density,
                })
            })
        })
    }

    /// `<width>x<height>-zoom<zoom>-<density>`, which is also its PNG's name.
    pub fn name(&self) -> String {
        let density: &str = self.density.into();
        format!(
            "{}x{}-zoom{}-{}",
            self.size.width,
            self.size.height,
            self.zoom,
            density.to_lowercase()
        )
    }
}
