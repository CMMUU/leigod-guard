//! Shared runtime brand assets. Update the source and run scripts/generate-brand.py.
pub const WINDOW_SIDE: usize = 256;
pub const WINDOW_RGBA: &[u8; WINDOW_SIDE * WINDOW_SIDE * 4] =
    include_bytes!("../assets/app-icon-256.rgba");
pub const TRAY_SIDE: usize = 32;
pub const TRAY_RGBA: &[u8; TRAY_SIDE * TRAY_SIDE * 4] =
    include_bytes!("../assets/app-icon-32.rgba");

pub fn window_icon() -> (Vec<u8>, u32, u32) {
    (WINDOW_RGBA.to_vec(), WINDOW_SIDE as u32, WINDOW_SIDE as u32)
}

pub fn tray_icon() -> (Vec<u8>, u32, u32) {
    (TRAY_RGBA.to_vec(), TRAY_SIDE as u32, TRAY_SIDE as u32)
}

/// Windows distinguishes the small window icon from the large taskbar icon.
/// ViewportBuilder::with_icon only sets the former in egui-winit 0.31.
pub fn native_options(mut options: eframe::NativeOptions) -> eframe::NativeOptions {
    use winit::platform::windows::WindowAttributesExtWindows;
    let (rgba, width, height) = window_icon();
    options.viewport = options.viewport.with_icon(egui::IconData {
        rgba: rgba.clone(),
        width,
        height,
    });
    let icon = winit::window::Icon::from_rgba(rgba, width, height)
        .expect("checked-in brand RGBA has a valid size");
    let previous = options.window_builder.take();
    options.window_builder = Some(Box::new(move |attributes| {
        let attributes = match previous {
            Some(build) => build(attributes),
            None => attributes,
        };
        attributes.with_taskbar_icon(Some(icon))
    }));
    options
}
