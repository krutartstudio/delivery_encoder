#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::anyhow;
use anyhow::Result;
use eframe::egui;
use egui::IconData;

mod app;
mod encoding;
mod models;
mod utils;

use app::DeliveryEncoderApp;

fn main() -> Result<()> {
    // Load icon data from assets
    let icon_bytes = include_bytes!("../assets/krutart.rgba");
    // We assume the icon is 256x256 (common icon size)
    let (icon_width, icon_height) = (256, 256);
    let icon_rgba = icon_bytes.to_vec(); // Convert to Vec<u8>

    let icon = IconData {
        rgba: icon_rgba,
        width: icon_width,
        height: icon_height,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([550.0, 450.0]) // Increased height for better spacing
            .with_icon(icon), // Set the icon here
        ..Default::default()
    };

    eframe::run_native(
        "Delivery Encoder",
        options,
        Box::new(|_| Box::new(DeliveryEncoderApp::new())),
    )
    .map_err(|e| anyhow!("Application error: {}", e))
}
