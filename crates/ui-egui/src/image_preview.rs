//! The raster image preview window: the canvas's right-click "Show Image" on a selected IMAGE
//! entity (`plan.image_show`, `context_menu.rs`'s `object_commands`) opens this instead of
//! running the command's plain JSON form, because the vector renderer only ever draws an
//! image's clip-boundary frame, never its pixels (`cmd::plan`'s doc comment says why) - this is
//! the only place in the app that actually shows what was imported.

use std::sync::atomic::{AtomicU32, Ordering};

use egui::TextureHandle;

use crate::CadApp;

/// Bumped each time `show` opens the window, so a second image (or the same one, re-edited)
/// replaces the cached texture instead of showing what was loaded before.
static GEN: AtomicU32 = AtomicU32::new(0);

#[derive(Clone)]
struct Loaded {
    generation: u32,
    shown: Result<TextureHandle, String>,
}

/// Open the preview window on `path` (an IMAGE entity's file, from `plan.image_show`'s reply).
pub fn show(app: &mut CadApp, path: &str) {
    GEN.fetch_add(1, Ordering::Relaxed);
    app.ui.image_preview_path = Some(path.to_string());
    app.ui.dialog = Some("imagepreview".into());
}

fn load(ctx: &egui::Context, path: &str) -> Result<TextureHandle, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("{path}: {e}"))?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Ok(ctx.load_texture("cc-image-preview", egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), egui::TextureOptions::LINEAR))
}

/// The Image Preview window: the file as it is on disk right now, with Close.
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let id = egui::Id::new("cc_image_preview");
    let generation = GEN.load(Ordering::Relaxed);
    let path = app.ui.image_preview_path.clone().unwrap_or_default();
    let cached = ctx.data(|d| d.get_temp::<Loaded>(id)).filter(|p| p.generation == generation);
    let loaded = match cached {
        Some(p) => p,
        None => {
            let p = Loaded { generation, shown: load(ctx, &path) };
            ctx.data_mut(|d| d.insert_temp(id, p.clone()));
            p
        }
    };
    let mut close = false;
    egui::Window::new(crate::tl!("Image Preview")).open(open).collapsible(false).resizable(true).show(ctx, |ui| {
        ui.label(&path);
        match &loaded.shown {
            Ok(tex) => {
                let size = tex.size_vec2();
                let k = (760.0_f32 / size.x).min(540.0 / size.y).min(1.0);
                ui.image((tex.id(), size * k));
            }
            Err(e) => {
                ui.label(e);
            }
        }
        ui.add_space(4.0);
        close = ui.button(crate::tl!("Close")).clicked();
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if close {
        *open = false;
    }
    if !*open {
        ctx.data_mut(|d| d.remove::<Loaded>(id));
        app.ui.image_preview_path = None;
    }
}
