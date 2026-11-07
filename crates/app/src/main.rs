//! Rust Fusion — a parametric 3D CAD application written in Rust (egui + wgpu).

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod app;
mod camera;
mod copilot;
mod export;
mod gen_ui;
mod image_model;
mod llm;
mod mcp;
mod panels;
mod render;
mod scene;
mod settings;
mod shots;
mod viewport;
mod views;
mod voice;
mod voice_ui;

fn main() -> eframe::Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--mcp-stdio") {
        let port = args
            .iter()
            .position(|a| a == "--port")
            .and_then(|i| args.get(i + 1))
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| settings::Settings::load().mcp_port);
        mcp::run_stdio_bridge(port);
        return Ok(());
    }
    let path = args.get(1).map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        depth_buffer: 24,
        multisampling: render::MSAA_SAMPLES as u16,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Rust Fusion")
            .with_inner_size([1400.0, 880.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Rust Fusion",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, path)))),
    )
}
