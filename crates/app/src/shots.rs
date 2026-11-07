//! Viewport screenshots for MCP clients and the image-to-model self-check.

use crate::app::App;
use crate::mcp::ToolResult;
use crossbeam_channel::Sender;
use eframe::egui;

pub enum ShotTarget {
    Mcp(Sender<ToolResult>),
    /// Image-to-model verification of one view (index into the view list).
    ImageCheck(usize),
}

pub fn request(app: &mut App, target: ShotTarget) {
    app.shots.push(target);
    if let Some(ctx) = &app.copilot.ctx {
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        ctx.request_repaint();
    }
}

/// Crop a full-window screenshot to `rect` (points) and encode as PNG.
pub fn crop_png(img: &egui::ColorImage, rect: egui::Rect, ppp: f32) -> Option<Vec<u8>> {
    let [w, h] = img.size;
    let x0 = ((rect.min.x * ppp).max(0.0) as usize).min(w);
    let y0 = ((rect.min.y * ppp).max(0.0) as usize).min(h);
    let x1 = ((rect.max.x * ppp) as usize).min(w);
    let y1 = ((rect.max.y * ppp) as usize).min(h);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let mut buf = image::RgbaImage::new((x1 - x0) as u32, (y1 - y0) as u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let c = img.pixels[y * w + x];
            buf.put_pixel((x - x0) as u32, (y - y0) as u32, image::Rgba([c.r(), c.g(), c.b(), 255]));
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(buf).write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

pub fn poll(app: &mut App) {
    if app.shots.is_empty() {
        return;
    }
    let Some(ctx) = app.copilot.ctx.clone() else { return };
    let shot = ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Screenshot { image, .. } => Some(image.clone()),
            _ => None,
        })
    });
    let Some(img) = shot else {
        ctx.request_repaint();
        return;
    };
    let png = crop_png(&img, app.viewport_rect, ctx.pixels_per_point());
    for t in std::mem::take(&mut app.shots) {
        match t {
            ShotTarget::Mcp(reply) => {
                let r = match &png {
                    Some(p) => ToolResult { text: "Viewport screenshot".into(), is_error: false, image_png: Some(p.clone()) },
                    None => ToolResult::err("screenshot failed"),
                };
                let _ = reply.send(r);
            }
            ShotTarget::ImageCheck(view) => crate::image_model::on_screenshot(app, view, png.clone()),
        }
    }
}
