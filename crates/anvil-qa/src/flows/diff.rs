//! Pixel comparison of screenshots against stored baselines.

use std::path::Path;

use anyhow::{Context, Result};
use image::{Rgba, RgbaImage};

use super::report::BaselineStatus;

/// Largest per-channel difference (0-255) still treated as the same colour (anti-aliasing noise).
pub const CHANNEL_TOLERANCE: u8 = 8;

/// Outcome of comparing two images of the same size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelDiff {
    pub changed: u64,
    pub total: u64,
}

impl PixelDiff {
    /// Share of changed pixels (0.0 to 1.0).
    pub fn ratio(&self) -> f64 {
        if self.total == 0 { 0.0 } else { self.changed as f64 / self.total as f64 }
    }
}

/// Counts the pixels whose channels differ by more than `tolerance`, and renders them in red over a
/// dimmed copy of `actual`. Returns `None` when the sizes differ.
pub fn diff_images(
    baseline: &RgbaImage,
    actual: &RgbaImage,
    tolerance: u8,
) -> Option<(PixelDiff, RgbaImage)> {
    if baseline.dimensions() != actual.dimensions() {
        return None;
    }
    let (width, height) = actual.dimensions();
    let mut canvas = RgbaImage::new(width, height);
    let mut changed = 0u64;
    for (x, y, new) in actual.enumerate_pixels() {
        let old = baseline.get_pixel(x, y);
        let moved = old.0.iter().zip(new.0.iter()).any(|(a, b)| a.abs_diff(*b) > tolerance);
        if moved {
            changed += 1;
            canvas.put_pixel(x, y, Rgba([255, 0, 0, 255]));
        } else {
            let [r, g, b, _] = new.0;
            canvas.put_pixel(x, y, Rgba([r / 3 + 170, g / 3 + 170, b / 3 + 170, 255]));
        }
    }
    Some((PixelDiff { changed, total: u64::from(width) * u64::from(height) }, canvas))
}

/// Compares the screenshot at `actual` with the PNG at `baseline`.
///
/// When any pixel changed, the diff image is written to `diff_out`. Changes up to `threshold`
/// (share of pixels) still count as a match.
pub fn compare_files(
    baseline: &Path,
    actual: &Path,
    threshold: f64,
    diff_out: &Path,
) -> Result<BaselineStatus> {
    let old = load_rgba(baseline)?;
    let new = load_rgba(actual)?;
    let Some((diff, canvas)) = diff_images(&old, &new, CHANNEL_TOLERANCE) else {
        return Ok(BaselineStatus::SizeMismatch {
            baseline: old.dimensions(),
            actual: new.dimensions(),
        });
    };
    let ratio = diff.ratio();
    if diff.changed > 0 {
        canvas
            .save(diff_out)
            .with_context(|| format!("failed to write diff image {}", diff_out.display()))?;
    }
    if ratio > threshold {
        return Ok(BaselineStatus::Diff { ratio, threshold, diff_path: diff_out.to_path_buf() });
    }
    Ok(BaselineStatus::Match { ratio })
}

fn load_rgba(path: &Path) -> Result<RgbaImage> {
    let img =
        image::open(path).with_context(|| format!("failed to read image {}", path.display()))?;
    Ok(img.to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(width, height, Rgba(color))
    }

    fn write(dir: &Path, name: &str, image: &RgbaImage) -> std::path::PathBuf {
        let path = dir.join(name);
        image.save(&path).expect("save png");
        path
    }

    #[test]
    fn identical_images_have_no_changed_pixels() {
        let a = solid(10, 10, [10, 20, 30, 255]);
        let (diff, _) = diff_images(&a, &a, CHANNEL_TOLERANCE).expect("same size");
        assert_eq!(diff, PixelDiff { changed: 0, total: 100 });
        assert!(diff.ratio().abs() < f64::EPSILON);
    }

    #[test]
    fn noise_below_the_channel_tolerance_is_ignored() {
        let a = solid(4, 4, [100, 100, 100, 255]);
        let b = solid(4, 4, [100 + CHANNEL_TOLERANCE, 100, 100, 255]);
        assert_eq!(diff_images(&a, &b, CHANNEL_TOLERANCE).map(|(d, _)| d.changed), Some(0));
        let c = solid(4, 4, [100 + CHANNEL_TOLERANCE + 1, 100, 100, 255]);
        assert_eq!(diff_images(&a, &c, CHANNEL_TOLERANCE).map(|(d, _)| d.changed), Some(16));
    }

    #[test]
    fn files_identical_small_change_and_large_change() {
        let dir = tempfile::tempdir().expect("tmp");
        let base = solid(20, 10, [255, 255, 255, 255]);
        let base_path = write(dir.path(), "base.png", &base);
        let diff_out = dir.path().join("diff.png");

        let same = write(dir.path(), "same.png", &base);
        let status = compare_files(&base_path, &same, 0.01, &diff_out).expect("compare");
        assert_eq!(status, BaselineStatus::Match { ratio: 0.0 });
        assert!(!diff_out.exists(), "no diff image for identical screenshots");

        let mut small = base.clone();
        small.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
        small.put_pixel(1, 0, Rgba([0, 0, 0, 255]));
        let small_path = write(dir.path(), "small.png", &small);
        let status = compare_files(&base_path, &small_path, 0.01, &diff_out).expect("compare");
        assert_eq!(status, BaselineStatus::Match { ratio: 0.01 });
        assert!(diff_out.exists(), "diff image is written whenever pixels changed");

        let mut large = base.clone();
        for x in 0..20 {
            large.put_pixel(x, 0, Rgba([0, 0, 0, 255]));
            large.put_pixel(x, 1, Rgba([0, 0, 0, 255]));
        }
        let large_path = write(dir.path(), "large.png", &large);
        let status = compare_files(&base_path, &large_path, 0.01, &diff_out).expect("compare");
        match status {
            BaselineStatus::Diff { ratio, threshold, diff_path } => {
                assert!((ratio - 0.2).abs() < 1e-9);
                assert!((threshold - 0.01).abs() < f64::EPSILON);
                assert_eq!(diff_path, diff_out);
            }
            other => panic!("expected a diff, got {other:?}"),
        }
    }

    #[test]
    fn size_mismatch_reports_both_sizes() {
        let dir = tempfile::tempdir().expect("tmp");
        let a = write(dir.path(), "a.png", &solid(20, 10, [0, 0, 0, 255]));
        let b = write(dir.path(), "b.png", &solid(20, 12, [0, 0, 0, 255]));
        let status = compare_files(&a, &b, 0.01, &dir.path().join("d.png")).expect("compare");
        assert_eq!(status, BaselineStatus::SizeMismatch { baseline: (20, 10), actual: (20, 12) });
        assert!(status.failed());
    }
}
