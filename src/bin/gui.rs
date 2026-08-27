//! nexo2ce-gui: native Windows desktop UI built on egui + eframe.
//!
//! Replaces the legacy local web GUI (legacy/web/). Target UX:
//! folder/ZIP input, batch queue with per-file status, diagnostics report
//! and CraftEngine ZIP export. This stub keeps the binary buildable while
//! the core port is in progress.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

struct ConverterApp;

impl eframe::App for ConverterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                ui.heading("Nexo → CraftEngine");
                ui.label("Rust 重写进行中：本地桌面 UI 将在此提供");
                ui.label("文件夹 / ZIP 输入 · 批量队列 · 诊断报告 · 导出 CraftEngine ZIP");
            });
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 580.0])
            .with_min_inner_size([640.0, 480.0])
            .with_title("Nexo → CraftEngine 转换器"),
        ..Default::default()
    };
    eframe::run_native(
        "Nexo → CraftEngine",
        options,
        Box::new(|_cc| Ok(Box::new(ConverterApp))),
    )
}
