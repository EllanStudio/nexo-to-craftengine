//! nexo2ce-gui: native Windows desktop UI built on egui + eframe.
//!
//! Replaces the legacy local web GUI (legacy/web/). Provides folder/ZIP
//! input, conversion options, a diagnostics report and CraftEngine ZIP
//! export. Conversion runs on a worker thread so the UI stays responsive.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use nexo2ce::converter::{convert, ConversionResult, ConvertOptions};
use nexo2ce::diagnostics::Severity;
use nexo2ce::{ClientMode, CmdPolicy};

enum JobState {
    Idle,
    Extracting,
    Running,
    Done,
    Failed(String),
}

struct Shared {
    state: JobState,
    result: Option<ConversionResult>,
}

#[derive(Clone, Copy, PartialEq)]
enum DiagFilter {
    All,
    Errors,
    Warnings,
    Lossy,
}

struct ConverterApp {
    input: String,
    output: String,
    namespace: String,
    client_mode: &'static str,
    cmd_policy: &'static str,
    strict: bool,
    force: bool,
    audit: bool,
    filter: DiagFilter,
    shared: Arc<Mutex<Shared>>,
    export_status: Option<String>,
}

impl Default for ConverterApp {
    fn default() -> Self {
        Self {
            input: String::new(),
            output: String::new(),
            namespace: String::new(),
            client_mode: "hybrid",
            cmd_policy: "preserve",
            strict: false,
            force: true,
            audit: true,
            filter: DiagFilter::All,
            shared: Arc::new(Mutex::new(Shared { state: JobState::Idle, result: None })),
            export_status: None,
        }
    }
}

fn extract_zip(zip_path: &Path, dest: &Path) -> Result<PathBuf, String> {
    let file = std::fs::File::open(zip_path).map_err(|error| format!("无法打开 ZIP: {}", error))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| format!("无效的 ZIP: {}", error))?;
    std::fs::create_dir_all(dest).map_err(|error| format!("无法创建临时目录: {}", error))?;
    archive.extract(dest).map_err(|error| format!("解压失败: {}", error))?;
    Ok(dest.to_path_buf())
}

fn zip_dir(src: &Path, dest: &Path) -> Result<u64, String> {
    let file = std::fs::File::create(dest).map_err(|error| format!("无法创建 ZIP: {}", error))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut count = 0u64;
    for entry in walkdir::WalkDir::new(src) {
        let entry = entry.map_err(|error| format!("遍历失败: {}", error))?;
        let path = entry.path();
        if path == src {
            continue;
        }
        let relative = path
            .strip_prefix(src)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            writer
                .add_directory(format!("{}/", relative), options)
                .map_err(|error| format!("写入目录失败: {}", error))?;
        } else {
            writer
                .start_file(relative, options)
                .map_err(|error| format!("写入文件失败: {}", error))?;
            let mut source = std::fs::File::open(path).map_err(|error| error.to_string())?;
            std::io::copy(&mut source, &mut writer).map_err(|error| error.to_string())?;
            count += 1;
        }
    }
    writer.finish().map_err(|error| format!("完成 ZIP 失败: {}", error))?;
    Ok(count)
}

impl ConverterApp {
    fn busy(&self) -> bool {
        let shared = self.shared.lock().unwrap();
        matches!(shared.state, JobState::Extracting | JobState::Running)
    }

    fn start_conversion(&mut self, ctx: &egui::Context) {
        let input = self.input.trim().to_string();
        let output = self.output.trim().to_string();
        if input.is_empty() || output.is_empty() {
            return;
        }
        let namespace = if self.namespace.trim().is_empty() {
            None
        } else {
            Some(self.namespace.trim().to_string())
        };
        let client_mode = ClientMode::parse(self.client_mode).unwrap_or(ClientMode::Hybrid);
        let cmd_policy = CmdPolicy::parse(self.cmd_policy).unwrap_or(CmdPolicy::Preserve);
        let (strict, force, audit) = (self.strict, self.force, self.audit);
        let shared = self.shared.clone();
        let ctx = ctx.clone();
        let after = ctx.clone();
        self.export_status = None;
        {
            let mut guard = shared.lock().unwrap();
            guard.state = if input.to_lowercase().ends_with(".zip") {
                JobState::Extracting
            } else {
                JobState::Running
            };
            guard.result = None;
        }
        std::thread::spawn(move || {
            let outcome = (|| -> Result<ConversionResult, String> {
                let input_root = if input.to_lowercase().ends_with(".zip") {
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_millis())
                        .unwrap_or(0);
                    let dest = std::env::temp_dir().join(format!("nexo2ce-gui-{}", stamp));
                    extract_zip(Path::new(&input), &dest)?
                } else {
                    PathBuf::from(&input)
                };
                {
                    let mut guard = shared.lock().unwrap();
                    guard.state = JobState::Running;
                }
                ctx.request_repaint();
                let options = ConvertOptions {
                    input: input_root.display().to_string(),
                    output,
                    namespace,
                    source_namespace: None,
                    client_mode,
                    cmd_policy,
                    strict,
                    force,
                    audit,
                };
                convert(&options).map_err(|error| error.to_string())
            })();
            let mut guard = shared.lock().unwrap();
            match outcome {
                Ok(result) => {
                    guard.state = JobState::Done;
                    guard.result = Some(result);
                }
                Err(message) => guard.state = JobState::Failed(message),
            }
            ctx.request_repaint();
        });
        after.request_repaint();
    }

    fn export_zip(&mut self) {
        let output = PathBuf::from(self.output.trim());
        if output.as_os_str().is_empty() {
            return;
        }
        let target = output.with_extension("zip");
        match zip_dir(&output, &target) {
            Ok(count) => {
                self.export_status = Some(format!("已导出 {} 个文件 → {}", count, target.display()));
            }
            Err(message) => self.export_status = Some(format!("导出失败: {}", message)),
        }
    }

    fn pick_input_folder(&mut self) {
        if let Some(folder) = rfd::FileDialog::new().set_title("选择 Nexo 目录").pick_folder() {
            self.input = folder.display().to_string();
        }
    }

    fn pick_input_zip(&mut self) {
        if let Some(file) = rfd::FileDialog::new()
            .set_title("选择 Nexo ZIP")
            .add_filter("ZIP", &["zip"])
            .pick_file()
        {
            self.input = file.display().to_string();
        }
    }

    fn pick_output(&mut self) {
        if let Some(folder) = rfd::FileDialog::new().set_title("选择 CraftEngine 输出目录").pick_folder() {
            self.output = folder.display().to_string();
        }
    }
}

impl eframe::App for ConverterApp {
    // egui/eframe 0.34+ trait method: the root Ui replaces the Context and
    // panels are shown inside it via the unified egui::Panel builder.
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        egui::Panel::top("header").resizable(false).show(root, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Nexo → CraftEngine 转换器");
                ui.label(format!("v{} · Rust 本地版", nexo2ce::VERSION));
            });
        });

        egui::Panel::left("form")
            .default_size(340.0)
            .min_size(340.0)
            .resizable(false)
            .show(root, |ui| {
            ui.add_space(8.0);
            ui.label("Nexo 输入（目录或 ZIP）");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.input).desired_width(f32::INFINITY));
            });
            ui.horizontal(|ui| {
                if ui.button("浏览目录…").clicked() {
                    self.pick_input_folder();
                }
                if ui.button("浏览 ZIP…").clicked() {
                    self.pick_input_zip();
                }
            });
            ui.add_space(6.0);
            ui.label("CraftEngine 输出目录");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.output).desired_width(f32::INFINITY));
                if ui.button("浏览…").clicked() {
                    self.pick_output();
                }
            });
            ui.add_space(10.0);
            ui.separator();
            ui.label("转换选项");
            ui.horizontal(|ui| {
                ui.label("命名空间");
                ui.add(egui::TextEdit::singleline(&mut self.namespace).hint_text("留空 = 自动检测").desired_width(160.0));
            });
            egui::ComboBox::from_label("客户端模式")
                .selected_text(self.client_mode)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.client_mode, "hybrid", "hybrid（推荐）");
                    ui.selectable_value(&mut self.client_mode, "modern", "modern");
                    ui.selectable_value(&mut self.client_mode, "legacy", "legacy");
                });
            egui::ComboBox::from_label("CMD 策略")
                .selected_text(self.cmd_policy)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.cmd_policy, "preserve", "preserve（推荐）");
                    ui.selectable_value(&mut self.cmd_policy, "allocate", "allocate");
                    ui.selectable_value(&mut self.cmd_policy, "omit", "omit");
                });
            ui.checkbox(&mut self.strict, "strict：存在 lossy 即视为失败");
            ui.checkbox(&mut self.force, "force：允许覆盖非空输出目录");
            ui.checkbox(&mut self.audit, "audit：模型/纹理资源图审计");
            ui.add_space(12.0);
            let busy = self.busy();
            let ready = !self.input.trim().is_empty() && !self.output.trim().is_empty() && !busy;
            if ui
                .add_enabled(ready, egui::Button::new("开始转换").min_size(egui::vec2(160.0, 32.0)))
                .clicked()
            {
                self.start_conversion(&ctx);
            }
            ui.add_space(6.0);
            let state_label = {
                let guard = self.shared.lock().unwrap();
                match &guard.state {
                    JobState::Idle => "就绪".to_string(),
                    JobState::Extracting => "正在解压 ZIP…".to_string(),
                    JobState::Running => "正在转换…".to_string(),
                    JobState::Done => "转换完成".to_string(),
                    JobState::Failed(message) => format!("失败: {}", message),
                }
            };
            ui.label(state_label);
            if busy {
                ui.spinner();
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        });

        egui::CentralPanel::default().show(root, |ui| {
            let has_result = self.shared.lock().unwrap().result.is_some();
            if !has_result {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.heading("诊断报告将显示在这里");
                    ui.label("选择 Nexo 目录或 ZIP 与输出目录后点击「开始转换」");
                });
                return;
            }
            let summary = {
                let guard = self.shared.lock().unwrap();
                let result = guard.result.as_ref().unwrap();
                let counts = result.diagnostics.counts();
                let audit = result.audit.as_ref().map(|audit| {
                    (
                        audit.resolved_models,
                        audit.referenced_models,
                        audit.generated_models,
                        audit.missing_models,
                        audit.resolved_textures,
                        audit.referenced_textures,
                        audit.missing_textures,
                        audit.referenced_blueprints,
                        audit.missing_blueprints,
                        audit.copied_item_definitions,
                    )
                });
                (
                    result.item_count,
                    result.category_count,
                    result.furniture_count,
                    result.block_count,
                    result.recipe_count,
                    result.sound_count,
                    result.glyph_count,
                    result.resource_count,
                    counts.info,
                    counts.warning,
                    counts.error,
                    counts.lossy,
                    audit,
                    result.report_file.clone(),
                )
            };
            ui.heading("转换结果");
            egui::Grid::new("summary").striped(true).show(ui, |ui| {
                ui.label("物品");
                ui.label(summary.0.to_string());
                ui.label("分类");
                ui.label(summary.1.to_string());
                ui.end_row();
                ui.label("家具");
                ui.label(summary.2.to_string());
                ui.label("方块");
                ui.label(summary.3.to_string());
                ui.end_row();
                ui.label("配方");
                ui.label(summary.4.to_string());
                ui.label("声音");
                ui.label(summary.5.to_string());
                ui.end_row();
                ui.label("字形图片");
                ui.label(summary.6.to_string());
                ui.label("资源文件");
                ui.label(summary.7.to_string());
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.label(format!(
                "诊断：{} 错误 · {} 警告 · {} lossy · {} 信息",
                summary.10, summary.9, summary.11, summary.8
            ));
            if let Some((
                resolved_models,
                referenced_models,
                generated_models,
                missing_models,
                resolved_textures,
                referenced_textures,
                missing_textures,
                referenced_blueprints,
                missing_blueprints,
                copied_item_definitions,
            )) = summary.12
            {
                ui.label(format!(
                    "审计：模型 {}/{} 解析（生成 {}，缺失 {}）· 纹理 {}/{} 解析（缺失 {}）· 蓝图 引用 {} 缺失 {} · 物品定义 {}",
                    resolved_models,
                    referenced_models,
                    generated_models,
                    missing_models,
                    resolved_textures,
                    referenced_textures,
                    missing_textures,
                    referenced_blueprints,
                    missing_blueprints,
                    copied_item_definitions
                ));
            }
            if let Some(report_file) = &summary.13 {
                ui.label(format!("报告: {}", report_file));
            }
            ui.horizontal(|ui| {
                if ui.button("导出 CraftEngine ZIP").clicked() {
                    self.export_zip();
                }
                if let Some(status) = &self.export_status {
                    ui.label(status.clone());
                }
            });
            ui.add_space(6.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("过滤");
                egui::ComboBox::from_id_salt("diag-filter")
                    .selected_text(match self.filter {
                        DiagFilter::All => "全部",
                        DiagFilter::Errors => "仅错误",
                        DiagFilter::Warnings => "仅警告",
                        DiagFilter::Lossy => "仅 lossy",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.filter, DiagFilter::All, "全部");
                        ui.selectable_value(&mut self.filter, DiagFilter::Errors, "仅错误");
                        ui.selectable_value(&mut self.filter, DiagFilter::Warnings, "仅警告");
                        ui.selectable_value(&mut self.filter, DiagFilter::Lossy, "仅 lossy");
                    });
            });
            let guard = self.shared.lock().unwrap();
            let result = guard.result.as_ref().unwrap();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for diagnostic in &result.diagnostics.items {
                    let visible = match self.filter {
                        DiagFilter::All => true,
                        DiagFilter::Errors => matches!(diagnostic.severity, Severity::Error),
                        DiagFilter::Warnings => matches!(diagnostic.severity, Severity::Warning),
                        DiagFilter::Lossy => diagnostic.lossy,
                    };
                    if !visible {
                        continue;
                    }
                    let color = match diagnostic.severity {
                        Severity::Error => egui::Color32::from_rgb(255, 120, 120),
                        Severity::Warning => egui::Color32::from_rgb(255, 200, 90),
                        Severity::Info => egui::Color32::from_rgb(140, 200, 255),
                    };
                    let mut location = String::new();
                    if let Some(source) = &diagnostic.source {
                        location.push_str(source);
                    }
                    if let Some(item) = &diagnostic.item {
                        if !location.is_empty() {
                            location.push_str(" › ");
                        }
                        location.push_str(item);
                    }
                    if let Some(field) = &diagnostic.field {
                        if !location.is_empty() {
                            location.push_str(" › ");
                        }
                        location.push_str(field);
                    }
                    let lossy_tag = if diagnostic.lossy { " [lossy]" } else { "" };
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(color, format!("[{}]", diagnostic.code));
                        ui.label(format!("{}{}", diagnostic.message, lossy_tag));
                        if !location.is_empty() {
                            ui.label(egui::RichText::new(location).weak());
                        }
                    });
                }
            });
        });
    }
}

/// egui's bundled default fonts have no CJK glyphs, which renders every
/// Chinese label as a tofu box. Append a system CJK font as the fallback for
/// both families (ASCII keeps the default look; CJK resolves to this font).
fn install_cjk_font(ctx: &egui::Context) {
    let candidates: &[&str] = &[
        // Windows: Microsoft YaHei (TTC face 0), then SimHei.
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyh.ttf",
        "C:\\Windows\\Fonts\\simhei.ttf",
        // Common Linux CJK fonts.
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
    ];
    for path in candidates {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("cjk".to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("cjk".to_owned());
        }
        ctx.set_fonts(fonts);
        return;
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 680.0])
            .with_min_inner_size([760.0, 540.0])
            .with_title("Nexo → CraftEngine 转换器"),
        ..Default::default()
    };
    eframe::run_native(
        "Nexo → CraftEngine",
        options,
        Box::new(|cc| {
            install_cjk_font(&cc.egui_ctx);
            Ok(Box::new(ConverterApp::default()))
        }),
    )
}