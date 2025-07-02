use anyhow::Result;
use anyhow::anyhow;
use eframe::egui;

mod app;
mod encoding;
mod models;
mod utils;

use app::DeliveryEncoderApp;

fn main() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([500.0, 350.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Delivery Encoder",
        options,
        Box::new(|_| Box::new(DeliveryEncoderApp::new())),
    )
    .map_err(|e| anyhow!("Application error: {}", e))
}
