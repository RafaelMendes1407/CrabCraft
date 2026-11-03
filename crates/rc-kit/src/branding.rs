//! One bundled mark for native windows, the project menu and About.
use egui::{Color32, ColorImage, Id, Rect, TextureHandle, TextureOptions, Ui, pos2};

pub const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon.png");

/// The window icon. macOS shows it in the Dock, so it gets the large one; on X11 the icon
/// travels in one request and 512 px (1 MiB) is over the limit, which leaves the window with
/// no icon at all, so elsewhere it is 128 px.
pub const WINDOW_ICON_PNG: &[u8] = if cfg!(target_os = "macos") { ICON_PNG } else { include_bytes!("../../../assets/icon-128.png") };

pub fn icon_data() -> egui::IconData {
    // The asset is generated and validated in the repository, never supplied by a user.
    eframe::icon_data::from_png_bytes(WINDOW_ICON_PNG).expect("bundled CrabCraft icon is a valid PNG")
}

pub fn paint(ui: &Ui, rect: Rect) {
    let (size, png): (u16, &[u8]) = if rect.width() * ui.ctx().pixels_per_point() <= 32.0 {
        (32, include_bytes!("../../../assets/icon-32.png"))
    } else {
        (64, include_bytes!("../../../assets/icon-64.png"))
    };
    let id = Id::new(("crabcraft_application_icon", size));
    let texture = ui
        .ctx()
        .data_mut(|data| data.get_temp::<TextureHandle>(id))
        .unwrap_or_else(|| {
            let icon = eframe::icon_data::from_png_bytes(png)
                .expect("bundled CrabCraft UI icon is a valid PNG");
            let image = ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            let texture = ui
                .ctx()
                .load_texture("CrabCraft", image, TextureOptions::LINEAR);
            ui.ctx()
                .data_mut(|data| data.insert_temp(id, texture.clone()));
            texture
        });
    ui.painter().image(
        texture.id(),
        rect,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_and_in_app_icon_decodes_with_transparent_padding() {
        let icon = super::icon_data();
        let size = if cfg!(target_os = "macos") { 512 } else { 128 };
        assert_eq!((icon.width, icon.height), (size, size));
        assert_eq!(icon.rgba.len() as u32, size * size * 4);
        assert_eq!(icon.rgba[3], 0);
        let middle = (size / 2 * size + size / 2) as usize;
        assert!(icon.rgba[middle * 4 + 3] >= 250);
        // X11 sends the icon (two header words and a 32-bit word per pixel) in one request,
        // capped at 256 KiB without the BIG-REQUESTS extension.
        #[cfg(target_os = "linux")]
        assert!((2 + icon.width * icon.height) * 4 <= 256 * 1024);
    }
}
