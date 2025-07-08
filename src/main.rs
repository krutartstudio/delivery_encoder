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
    let icon_bytes = include_bytes!("../assets/krutart.rgba");

    let (icon_width, icon_height) = (256, 256);
    let icon_rgba = icon_bytes.to_vec();

    let icon = IconData {
        rgba: icon_rgba,
        width: icon_width,
        height: icon_height,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([565.0, 580.0])
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "Delivery Encoder",
        options,
        Box::new(|_| Box::new(DeliveryEncoderApp::new())),
    )
    .map_err(|e| anyhow!("Application error: {}", e))
}