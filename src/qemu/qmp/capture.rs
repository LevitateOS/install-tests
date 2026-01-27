//! Screen capture via QMP screendump.
//!
//! Captures screenshots from QEMU for visual verification.

use crate::qemu::qmp::QmpClient;
use anyhow::Result;
use std::path::Path;

/// Capture a screenshot and save to file.
///
/// Screenshots are saved in PPM format, which can be converted
/// to PNG using ImageMagick: `convert screen.ppm screen.png`
///
/// # Arguments
/// * `client` - QMP client connection
/// * `filename` - Path to save the screenshot (PPM format)
pub fn screendump(client: &mut QmpClient, filename: &str) -> Result<()> {
    client.screendump(filename)
}

/// Capture a screenshot and check if a pattern appears.
///
/// This is a placeholder for future OCR integration.
/// Currently just captures the screenshot for manual inspection.
///
/// # Arguments
/// * `client` - QMP client connection
/// * `filename` - Path to save the screenshot
/// * `_pattern` - Text pattern to look for (currently unused)
pub fn capture_and_check(
    client: &mut QmpClient,
    filename: &str,
    _pattern: &str,
) -> Result<bool> {
    screendump(client, filename)?;

    // For now, just capture - OCR integration would go here
    // Future: Use tesseract or similar to extract text and match pattern
    Ok(true)
}

/// Capture screenshots in a sequence for debugging.
///
/// Useful for step-by-step visual debugging.
///
/// # Arguments
/// * `client` - QMP client connection
/// * `base_path` - Directory to save screenshots
/// * `prefix` - Prefix for screenshot filenames
/// * `index` - Sequence number
pub fn capture_sequence<P: AsRef<Path>>(
    client: &mut QmpClient,
    base_path: P,
    prefix: &str,
    index: usize,
) -> Result<String> {
    let filename = base_path
        .as_ref()
        .join(format!("{}_{:04}.ppm", prefix, index))
        .to_string_lossy()
        .to_string();

    screendump(client, &filename)?;
    Ok(filename)
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_sequence_filename_format() {
        // Just verify the format logic
        let base = std::path::Path::new("/tmp");
        let filename = base
            .join(format!("{}_{:04}.ppm", "screen", 42))
            .to_string_lossy()
            .to_string();
        assert_eq!(filename, "/tmp/screen_0042.ppm");
    }
}
