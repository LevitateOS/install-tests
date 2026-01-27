//! Screen capture via QMP screendump.
//!
//! Captures screenshots from QEMU for visual verification.

use crate::qemu::qmp::QmpClient;
use anyhow::Result;

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
