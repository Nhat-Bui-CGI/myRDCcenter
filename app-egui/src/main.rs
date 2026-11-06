// myRDCcenter, egui front-end.
//
// Anti-flicker notes (egui can "blink" on Windows if left at defaults):
// - glow renderer with vsync, no MSAA, no transparency;
// - window stays hidden until the first frame is painted (eframe default) and
//   uses a fixed background matching the light panels so there is no flash on start/resize;
// - no continuous repaint: we only repaint on input, plus one slow timer for auto-lock;
// - every container has a stable id so egui never has to re-measure (sizing pass)
//   when the selection changes, which is what makes grids flash for a frame;
// - the form keeps a fixed column width so the layout doesn't jump while typing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use myrdc_core::prefs::{Prefs, TextSizeKey};
use myrdc_core::theme::{self, Rgb};
use myrdc_core::{default_vault_path, export, groups, import, keystore, rdp, Connection, TreeRow, Vault};
use uuid::Uuid;

const LABEL_WIDTH: f32 = 130.0;
const FIELD_WIDTH: f32 = 320.0;

fn main() -> eframe::Result {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(concat!("myRDCcenter ", env!("CARGO_PKG_VERSION")))
            .with_inner_size(window_size())
            .with_min_inner_size([720.0, 440.0])
            .with_transparent(false)
            .with_icon(egui::IconData {
                rgba: include_bytes!("../../assets/icon-128.rgba").to_vec(),
                width: 128,
                height: 128,
            }),
        renderer: eframe::Renderer::Glow,
        multisampling: 0,
        centered: true,
        dithering: false,
        ..Default::default()
    };
    options.glow_options.vsync = true;
    eframe::run_native("myRDCcenter", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

enum Screen {
    Locked(LockScreen),
    Open(Box<OpenVault>),
}

struct LockScreen {
    password: String,
    confirm: String,
    error: String,
    /// "Remember on this computer" checkbox.
    remember: bool,
    /// A remembered key exists, so "Unlock with my login" is offered.
    has_remembered: bool,
}

impl LockScreen {
    fn new(path: &PathBuf) -> Self {
        Self {
            password: String::new(),
            confirm: String::new(),
            error: String::new(),
            remember: false,
            has_remembered: keystore::load(path).is_some(),
        }
    }
}

struct OpenVault {
    vault: Vault,
    search: String,
    selected: Option<Uuid>,
    /// Working copy of the selected connection; written back on Save.
    draft: Option<Connection>,
    show_password: bool,
    status: String,
    confirm_delete: bool,
    change_pw: Option<(String, String)>,
    /// The Settings dialog is open.
    settings_open: bool,
    remembered: bool,
    /// Group selected in the tree (when no connection is selected).
    selected_group: Option<String>,
    group_dialog: Option<GroupDialog>,
    export_dialog: Option<ExportDialog>,
    /// .rdcx files waiting for their export password.
    import_pw_dialog: Option<ImportPwDialog>,
    confirm_delete_group: bool,
}

/// "New group" / "Rename group" dialog.
struct GroupDialog {
    /// None = create a new group under `parent`; Some(path) = rename that group.
    renaming: Option<String>,
    parent: String,
    name: String,
    error: String,
}

/// "Export" dialog: what to export and the password that locks the file.
struct ExportDialog {
    /// None = everything; Some(path) = this group and its subgroups.
    group: Option<String>,
    password: String,
    confirm: String,
    /// Remove the exported connections here afterwards ("move" instead of "copy").
    move_after: bool,
    error: String,
}

struct ImportPwDialog {
    files: Vec<PathBuf>,
    password: String,
    error: String,
}

/// Something the user did in the tree, applied after drawing it.
enum TreeAction {
    SelectConnection(Uuid),
    Connect(Uuid),
    DeleteConnection(Uuid),
    Move(Uuid, String),
    ClickGroup(String),
    ToggleGroup(String),
    NewConnectionIn(String),
    NewGroupIn(String),
    RenameGroup(String),
    DeleteGroup(String),
    ExportGroup(String),
}

struct App {
    vault_path: PathBuf,
    screen: Screen,
    last_activity: Instant,
    /// Settings of this computer (text size).
    prefs: Prefs,
    /// Frames drawn so far; a screenshot waits a few frames for the layout to settle.
    #[cfg(feature = "screenshot")]
    frames: u32,
}

fn window_size() -> [f32; 2] {
    #[cfg(feature = "screenshot")]
    if myrdc_core::screenshot::request().is_some() {
        let (w, h) = myrdc_core::screenshot::SIZE;
        return [w, h];
    }
    [1000.0, 640.0]
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Light);
        cc.egui_ctx.all_styles_mut(|style| {
            // No fade animations: they are a common source of visible flicker.
            style.animation_time = 0.0;
            apply_theme(style);
        });
        // Our own Ctrl +/-/0 handling, with the same steps as the Slint app.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let prefs = Prefs::load();
        cc.egui_ctx.set_zoom_factor(prefs.zoom());
        let vault_path = default_vault_path();
        // Open straight away when the key is remembered on this computer.
        let screen = match Vault::open_remembered(&vault_path) {
            Some(Ok(v)) => Screen::Open(Box::new(OpenVault::new(v))),
            _ => Screen::Locked(LockScreen::new(&vault_path)),
        };
        Self {
            vault_path,
            screen,
            last_activity: Instant::now(),
            prefs,
            #[cfg(feature = "screenshot")]
            frames: 0,
        }
    }

    fn lock(&mut self) {
        self.screen = Screen::Locked(LockScreen::new(&self.vault_path));
    }

    fn auto_lock_check(&mut self, ctx: &egui::Context) {
        let had_input = ctx.input(|i| !i.events.is_empty() || i.pointer.is_moving());
        if had_input {
            self.last_activity = Instant::now();
        }
        if let Screen::Open(open) = &self.screen {
            let minutes = open.vault.data.settings.auto_lock_minutes;
            if minutes > 0 {
                if self.last_activity.elapsed() >= Duration::from_secs(minutes as u64 * 60) {
                    self.lock();
                } else {
                    // One cheap wake-up every few seconds, not a repaint loop.
                    ctx.request_repaint_after(Duration::from_secs(5));
                }
            }
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Same as the light panel fill, so a resize never shows a different colour.
        col(theme::BG).to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.auto_lock_check(&ctx);
        let mut text_size = text_size_shortcut(&ctx);

        let mut lock_now = false;
        let mut unlocked = None;
        match &mut self.screen {
            Screen::Locked(lock) => unlocked = lock_ui(ui, lock, &self.vault_path),
            Screen::Open(open) => {
                let label = self.prefs.text_size_label();
                lock_now = open.ui(ui, &label, &mut text_size);
            }
        }
        if let Some(key) = text_size {
            if key.apply(&mut self.prefs) {
                ctx.set_zoom_factor(self.prefs.zoom());
            }
        }
        if let Some(vault) = unlocked {
            self.last_activity = Instant::now();
            self.screen = Screen::Open(Box::new(OpenVault::new(vault)));
        }
        if lock_now {
            self.lock();
        }
        #[cfg(feature = "screenshot")]
        self.screenshot_step(&ctx);
    }
}

/// Ctrl (Cmd on macOS) with + / = / - / 0 changes the text size.
fn text_size_shortcut(ctx: &egui::Context) -> Option<TextSizeKey> {
    let cmd = egui::Modifiers::COMMAND;
    ctx.input_mut(|i| {
        if i.consume_key(cmd, egui::Key::Plus) || i.consume_key(cmd, egui::Key::Equals) {
            Some(TextSizeKey::Larger)
        } else if i.consume_key(cmd, egui::Key::Minus) {
            Some(TextSizeKey::Smaller)
        } else if i.consume_key(cmd, egui::Key::Num0) {
            Some(TextSizeKey::Reset)
        } else {
            None
        }
    })
}

#[cfg(feature = "screenshot")]
impl App {
    /// Select the requested connection, wait for the layout, then save a PNG and exit.
    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(req) = myrdc_core::screenshot::request() else { return };
        self.frames += 1;
        if self.frames == 1 {
            if let (Screen::Open(open), Some(name)) = (&mut self.screen, &req.select) {
                if let Some(id) = open.vault.data.connections.iter().find(|c| &c.name == name).map(|c| c.id) {
                    open.select(id);
                }
            }
        }
        if self.frames == 6 {
            ctx.request_screenshot(move |img| {
                let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                let res = myrdc_core::screenshot::save_png(&req.out, img.size[0] as u32, img.size[1] as u32, &rgba);
                if let Err(e) = res {
                    eprintln!("Could not save screenshot: {e}");
                }
                std::process::exit(0);
            });
        }
        ctx.request_repaint();
    }
}

fn lock_ui(ui: &mut egui::Ui, lock: &mut LockScreen, path: &PathBuf) -> Option<Vault> {
    let creating = !Vault::exists(path);
    let mut result = None;
    egui::CentralPanel::default().frame(egui::Frame::new().fill(col(theme::BG))).show(ui, |ui| {
        ui.add_space(ui.available_height() * 0.18);
        ui.vertical_centered(|ui| {
          card().inner_margin(egui::Margin::same(28)).show(ui, |ui| {
           ui.set_width(340.0);
           ui.vertical_centered(|ui| {
            logo(ui, 48.0);
            ui.add_space(6.0);
            ui.label(RichText::new("myRDCcenter").size(28.0).strong().color(col(theme::TEXT)));
            ui.label(RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION"))).weak());
            ui.add_space(8.0);
            ui.label(if creating {
                "Create a master password. It encrypts your vault and can't be recovered."
            } else {
                "Enter your master password to unlock."
            });
            ui.add_space(12.0);
            let pw = ui.add_sized(
                [280.0, 24.0],
                egui::TextEdit::singleline(&mut lock.password).password(true).hint_text("Master password").id_salt("master_pw"),
            );
            let mut submit = pw.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if creating {
                let c = ui.add_sized(
                    [280.0, 24.0],
                    egui::TextEdit::singleline(&mut lock.confirm).password(true).hint_text("Repeat password").id_salt("master_pw2"),
                );
                submit |= c.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            }
            if keystore::SUPPORTED {
                ui.checkbox(&mut lock.remember, keystore::label());
            }
            ui.add_space(8.0);
            submit |= ui.add_sized([280.0, 32.0], filled_button(if creating { "Create vault" } else { "Unlock" }, theme::ACCENT)).clicked();
            if lock.has_remembered && !creating && ui.add_sized([280.0, 28.0], egui::Button::new("Unlock with my computer login")).clicked() {
                match Vault::open_remembered(path) {
                    Some(Ok(v)) => result = Some(v),
                    Some(Err(e)) => lock.error = e.to_string(),
                    None => {
                        lock.has_remembered = false;
                        lock.error = "No remembered key found. Use the master password.".into();
                    }
                }
            }
            // Fixed-height error line, so the layout doesn't jump when an error shows up.
            ui.add_sized([300.0, 20.0], egui::Label::new(RichText::new(&lock.error).color(col(theme::DANGER))));
            ui.label(RichText::new(path.display().to_string()).small().weak());

            if submit {
                let res = if creating {
                    if lock.password.len() < 8 {
                        Err("Use at least 8 characters".to_string())
                    } else if lock.password != lock.confirm {
                        Err("Passwords don't match".to_string())
                    } else {
                        Vault::create(path, &lock.password).map_err(|e| e.to_string())
                    }
                } else {
                    Vault::open(path, &lock.password).map_err(|e| e.to_string())
                };
                match res {
                    Ok(v) => {
                        if lock.remember {
                            if let Err(e) = v.set_remembered(true) {
                                lock.error = e;
                            }
                        }
                        result = Some(v);
                    }
                    Err(e) => lock.error = e,
                }
                wipe(&mut lock.password);
                wipe(&mut lock.confirm);
            }
            // Focus the password box on start, but never steal focus from the repeat box.
            if ui.memory(|m| m.focused().is_none()) {
                pw.request_focus();
            }
           });
          });
        });
    });
    result
}

fn wipe(s: &mut String) {
    // Overwrite before clearing so the password doesn't linger in memory.
    unsafe { s.as_bytes_mut().fill(0) };
    s.clear();
}

impl OpenVault {
    fn new(vault: Vault) -> Self {
        let remembered = vault.is_remembered();
        Self {
            vault,
            search: String::new(),
            selected: None,
            draft: None,
            show_password: false,
            status: String::new(),
            confirm_delete: false,
            change_pw: None,
            settings_open: false,
            remembered,
            selected_group: None,
            group_dialog: None,
            export_dialog: None,
            import_pw_dialog: None,
            confirm_delete_group: false,
        }
    }

    fn select(&mut self, id: Uuid) {
        self.selected_group = None;
        self.selected = Some(id);
        self.draft = self.vault.data.find(id).cloned();
        self.show_password = false;
        self.confirm_delete = false;
    }

    fn save(&mut self) {
        if let Some(d) = &self.draft {
            match self.vault.data.find_mut(d.id) {
                Some(c) => *c = d.clone(),
                None => self.vault.data.connections.push(d.clone()),
            }
        }
        self.status = match self.vault.save() {
            Ok(()) => "Saved".into(),
            Err(e) => e.to_string(),
        };
    }

    fn connect(&mut self, c: &Connection) {
        self.status = match rdp::launch(c) {
            Ok(()) => format!("Connecting to {}…", c.display_name()),
            Err(e) => e,
        };
    }

    /// Returns true when the user asked to lock.
    /// `text_size_label` is shown in the toolbar; a click on its buttons sets `text_size`.
    fn ui(&mut self, ui: &mut egui::Ui, text_size_label: &str, text_size: &mut Option<TextSizeKey>) -> bool {
        let mut lock = false;

        let bar = egui::Frame::new().fill(col(theme::PANEL)).inner_margin(egui::Margin::symmetric(10, 8));
        egui::Panel::top("toolbar").frame(bar).show(ui, |ui| {
            ui.horizontal(|ui| {
                logo(ui, 22.0);
                ui.label(RichText::new("myRDCcenter").strong().size(15.0).color(col(theme::TEXT)));
                ui.add_space(8.0);
                if ui.add(filled_button("+ New connection", theme::ACCENT)).clicked() {
                    let group = self.current_group();
                    self.new_connection(&group);
                }
                if ui.button("New group").clicked() {
                    let group = self.current_group();
                    self.open_new_group(&group);
                }
                if ui.button("Import").on_hover_text("Import .rdp or .rdm files").clicked() {
                    self.import();
                }
                if ui.button("Export").on_hover_text("Copy or move connections to another computer").clicked() {
                    self.open_export(None);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(outline_button("Lock", theme::DANGER)).clicked() {
                        lock = true;
                    }
                    if ui.button("Settings").on_hover_text("Backup, master password, auto-lock").clicked() {
                        self.settings_open = true;
                    }
                    ui.separator();
                    if ui.small_button("A+").on_hover_text("Larger text (Ctrl +)").clicked() {
                        *text_size = Some(TextSizeKey::Larger);
                    }
                    if ui.small_button(text_size_label).on_hover_text("Text size. Click for 100 % (Ctrl 0)").clicked() {
                        *text_size = Some(TextSizeKey::Reset);
                    }
                    if ui.small_button("A-").on_hover_text("Smaller text (Ctrl -)").clicked() {
                        *text_size = Some(TextSizeKey::Smaller);
                    }
                });
            });
        });

        let status = egui::Frame::new().fill(col(theme::PANEL)).inner_margin(egui::Margin::symmetric(10, 3));
        egui::Panel::bottom("status").frame(status).show(ui, |ui| {
            ui.horizontal(|ui| {
                if !self.status.is_empty() {
                    dot(ui, theme::status_color(&self.status), 8.0);
                }
                ui.add_sized([ui.available_width(), 18.0], egui::Label::new(RichText::new(&self.status).color(col(theme::TEXT_WEAK))).truncate());
            });
        });

        let side = egui::Frame::new().fill(col(theme::PANEL)).inner_margin(egui::Margin::symmetric(8, 8));
        egui::Panel::left("list").frame(side).resizable(true).default_size(280.0).size_range(200.0..=500.0).show(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search").desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 5)).id_salt("search"));
            ui.add_space(6.0);
            self.list_ui(ui);
        });

        let main = egui::Frame::new().fill(col(theme::BG)).inner_margin(egui::Margin::same(16));
        egui::CentralPanel::default().frame(main).show(ui, |ui| self.detail_ui(ui));

        self.settings_ui(ui);
        self.change_password_ui(ui);
        self.group_dialog_ui(ui);
        self.export_dialog_ui(ui);
        self.import_pw_dialog_ui(ui);
        lock
    }

    /// Group that new items go into: the selected group, or the selected connection's group.
    fn current_group(&self) -> String {
        if let Some(g) = &self.selected_group {
            return g.clone();
        }
        self.draft.as_ref().map(|d| groups::normalize(&d.group)).unwrap_or_default()
    }

    fn new_connection(&mut self, group: &str) {
        let c = Connection::new("New connection", group);
        self.selected_group = None;
        self.selected = Some(c.id);
        self.draft = Some(c);
        self.confirm_delete = false;
    }

    fn open_new_group(&mut self, parent: &str) {
        self.group_dialog = Some(GroupDialog { renaming: None, parent: parent.to_string(), name: String::new(), error: String::new() });
    }

    fn select_group(&mut self, path: &str) {
        self.selected_group = Some(path.to_string());
        self.selected = None;
        self.draft = None;
        self.confirm_delete = false;
        self.confirm_delete_group = false;
    }

    fn list_ui(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
        let rows = self.vault.data.tree_rows(&self.search);
        let colors = theme::row_colors(&rows);
        let all_groups = self.vault.data.all_groups();
        egui::ScrollArea::vertical().id_salt("list_scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (row, &color) in rows.iter().zip(&colors) {
                    match row {
                        TreeRow::Group { path, name, depth, expanded, count } => {
                            let selected = self.selected_group.as_deref() == Some(path.as_str());
                            ui.horizontal(|ui| {
                                ui.add_space(*depth as f32 * 16.0);
                                if triangle(ui, *expanded).clicked() {
                                    action = Some(TreeAction::ToggleGroup(path.clone()));
                                }
                                swatch(ui, color);
                                let label = RichText::new(name.as_str()).strong().color(col(theme::TEXT));
                                let r = ui.add(egui::Button::selectable(selected, label));
                                count_badge(ui, *count, color);
                                if r.clicked() {
                                    action = Some(TreeAction::ClickGroup(path.clone()));
                                }
                                r.context_menu(|ui| {
                                    if ui.button("New connection here").clicked() {
                                        action = Some(TreeAction::NewConnectionIn(path.clone()));
                                    }
                                    if ui.button("New subgroup").clicked() {
                                        action = Some(TreeAction::NewGroupIn(path.clone()));
                                    }
                                    if ui.button("Rename / move").clicked() {
                                        action = Some(TreeAction::RenameGroup(path.clone()));
                                    }
                                    if ui.button("Export group...").clicked() {
                                        action = Some(TreeAction::ExportGroup(path.clone()));
                                    }
                                    if ui.button("Delete group").clicked() {
                                        action = Some(TreeAction::DeleteGroup(path.clone()));
                                    }
                                });
                            });
                        }
                        TreeRow::Connection { id, name, address, depth } => {
                            let selected = self.selected == Some(*id);
                            ui.horizontal(|ui| {
                                // Line up with group names (indent + triangle width).
                                ui.add_space(*depth as f32 * 16.0 + 18.0);
                                dot(ui, color, 8.0);
                                let r = ui.add(egui::Button::selectable(selected, name.as_str())).on_hover_text(address);
                                if r.clicked() {
                                    action = Some(TreeAction::SelectConnection(*id));
                                }
                                if r.double_clicked() {
                                    action = Some(TreeAction::Connect(*id));
                                }
                                r.context_menu(|ui| {
                                    if ui.button("Connect").clicked() {
                                        action = Some(TreeAction::Connect(*id));
                                    }
                                    ui.menu_button("Move to group", |ui| {
                                        if ui.button("(no group)").clicked() {
                                            action = Some(TreeAction::Move(*id, String::new()));
                                        }
                                        for g in &all_groups {
                                            if ui.button(g).clicked() {
                                                action = Some(TreeAction::Move(*id, g.clone()));
                                            }
                                        }
                                    });
                                    if ui.button("Delete").clicked() {
                                        action = Some(TreeAction::DeleteConnection(*id));
                                    }
                                });
                            });
                        }
                    }
                }
            });
            if rows.is_empty() {
                let text = if self.search.is_empty() { "No connections yet. Click New connection." } else { "Nothing found." };
                ui.label(RichText::new(text).weak());
            }
        });
        if let Some(a) = action {
            self.apply(a);
        }
    }

    fn apply(&mut self, action: TreeAction) {
        match action {
            TreeAction::SelectConnection(id) => self.select(id),
            TreeAction::Connect(id) => {
                self.select(id);
                if let Some(c) = self.vault.data.find(id).cloned() {
                    self.connect(&c);
                }
            }
            TreeAction::DeleteConnection(id) => {
                self.select(id);
                self.confirm_delete = true;
            }
            TreeAction::Move(id, group) => {
                self.vault.data.move_connection(id, &group);
                if let Some(d) = self.draft.as_mut().filter(|d| d.id == id) {
                    d.group = group.clone();
                }
                self.save_vault(&format!("Moved to {}", if group.is_empty() { "(no group)" } else { &group }));
            }
            TreeAction::ClickGroup(path) => {
                if self.selected_group.as_deref() == Some(path.as_str()) {
                    let collapsed = self.vault.data.is_collapsed(&path);
                    self.vault.data.set_collapsed(&path, !collapsed);
                    let _ = self.vault.save();
                } else {
                    self.select_group(&path);
                    if self.vault.data.is_collapsed(&path) {
                        self.vault.data.set_collapsed(&path, false);
                        let _ = self.vault.save();
                    }
                }
            }
            TreeAction::ToggleGroup(path) => {
                let collapsed = self.vault.data.is_collapsed(&path);
                self.vault.data.set_collapsed(&path, !collapsed);
                let _ = self.vault.save();
            }
            TreeAction::NewConnectionIn(path) => self.new_connection(&path),
            TreeAction::NewGroupIn(path) => self.open_new_group(&path),
            TreeAction::RenameGroup(path) => {
                self.group_dialog = Some(GroupDialog {
                    renaming: Some(path.clone()),
                    parent: String::new(),
                    name: path,
                    error: String::new(),
                });
            }
            TreeAction::ExportGroup(path) => self.open_export(Some(path)),
            TreeAction::DeleteGroup(path) => {
                self.select_group(&path);
                self.confirm_delete_group = true;
            }
        }
    }

    fn save_vault(&mut self, ok_message: &str) {
        self.status = match self.vault.save() {
            Ok(()) => ok_message.to_string(),
            Err(e) => e.to_string(),
        };
    }

    fn group_ui(&mut self, ui: &mut egui::Ui, path: String) {
        let count = self.vault.data.count_in(&path);
        let color = theme::group_color(&path);
        ui.horizontal(|ui| {
            avatar(ui, groups::leaf(&path), color);
            ui.vertical(|ui| {
                ui.label(RichText::new(groups::leaf(&path)).size(22.0).strong().color(col(theme::TEXT)));
                ui.label(RichText::new(&path).color(col(color)));
            });
        });
        ui.add_space(4.0);
        ui.label(format!("{count} connection(s) in this group and its subgroups."));
        ui.add_space(12.0);
        let mut action = None;
        ui.horizontal(|ui| {
            if ui.add(filled_button("+ New connection here", theme::ACCENT)).clicked() {
                action = Some(TreeAction::NewConnectionIn(path.clone()));
            }
            if ui.button("New subgroup").clicked() {
                action = Some(TreeAction::NewGroupIn(path.clone()));
            }
            if ui.button("Rename / move").clicked() {
                action = Some(TreeAction::RenameGroup(path.clone()));
            }
            if ui.button("Export group").clicked() {
                action = Some(TreeAction::ExportGroup(path.clone()));
            }
            if self.confirm_delete_group {
                if ui.add(filled_button("Really delete group?", theme::DANGER)).clicked() {
                    self.vault.data.delete_group(&path);
                    self.selected_group = None;
                    self.confirm_delete_group = false;
                    self.save_vault("Group deleted. Its connections moved up one level.");
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_delete_group = false;
                }
            } else if ui.add(outline_button("Delete group", theme::DANGER)).clicked() {
                self.confirm_delete_group = true;
            }
        });
        if self.confirm_delete_group {
            ui.add_space(6.0);
            ui.label(RichText::new("Connections in this group are kept and move up one level.").weak());
        }
        if let Some(a) = action {
            self.apply(a);
        }
    }

    fn group_dialog_ui(&mut self, ui: &mut egui::Ui) {
        let Some(dlg) = self.group_dialog.as_mut() else { return };
        let mut close = false;
        let mut apply = false;
        egui::Modal::new(egui::Id::unique("group_dialog")).show(ui.ctx(), |ui| {
            ui.set_width(340.0);
            match &dlg.renaming {
                Some(old) => {
                    ui.heading("Rename group");
                    ui.label(RichText::new(format!("Use / to move it, e.g. Work/{}", groups::leaf(old))).weak());
                }
                None => {
                    ui.heading("New group");
                    if !dlg.parent.is_empty() {
                        ui.label(RichText::new(format!("Inside {}", dlg.parent)).weak());
                    }
                }
            }
            ui.add_space(8.0);
            let r = ui.add(egui::TextEdit::singleline(&mut dlg.name).hint_text("Group name").desired_width(f32::INFINITY).id_salt("group_name"));
            if !r.has_focus() && dlg.name.is_empty() {
                r.request_focus();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                apply = true;
            }
            ui.add_sized([320.0, 18.0], egui::Label::new(RichText::new(&dlg.error).color(col(theme::DANGER))));
            ui.horizontal(|ui| {
                apply |= ui.add(filled_button("OK", theme::ACCENT)).clicked();
                close = ui.button("Cancel").clicked();
            });
        });
        if apply {
            let name = groups::normalize(&dlg.name);
            if name.is_empty() {
                dlg.error = "Enter a name".into();
            } else {
                match dlg.renaming.clone() {
                    Some(old) => match self.vault.data.rename_group(&old, &name) {
                        Ok(()) => {
                            if let Some(d) = self.draft.as_mut() {
                                if groups::is_within(&d.group, &old) {
                                    d.group = format!("{name}{}", &d.group[old.len()..]);
                                }
                            }
                            self.selected_group = Some(name.clone());
                            self.save_vault("Group renamed");
                            close = true;
                        }
                        Err(e) => self.group_dialog.as_mut().unwrap().error = e,
                    },
                    None => {
                        let full = if dlg.parent.is_empty() { name } else { format!("{}/{name}", dlg.parent) };
                        let path = self.vault.data.add_group(&full);
                        self.vault.data.set_collapsed(&dlg.parent, false);
                        self.select_group(&path);
                        self.save_vault("Group created");
                        close = true;
                    }
                }
            }
        }
        if close {
            self.group_dialog = None;
        }
    }

    fn detail_ui(&mut self, ui: &mut egui::Ui) {
        if self.draft.is_none() {
            if let Some(path) = self.selected_group.clone() {
                self.group_ui(ui, path);
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Select a connection, or double-click to connect").size(15.0).color(col(theme::TEXT_WEAK)))
                });
            }
            return;
        }
        let all_groups = self.vault.data.all_groups();
        let Some(d) = self.draft.as_mut() else { return };
        let mut do_save = false;
        let mut do_connect = false;
        let mut do_delete = false;

        egui::ScrollArea::vertical().id_salt("detail_scroll").auto_shrink([false, false]).show(ui, |ui| {
            let color = theme::group_color(&d.group);
            ui.horizontal(|ui| {
                avatar(ui, d.display_name(), color);
                ui.vertical(|ui| {
                    ui.label(RichText::new(d.display_name().to_string()).size(22.0).strong().color(col(theme::TEXT)));
                    let sub = if d.group.is_empty() { d.address() } else { format!("{}  \u{b7}  {}", d.group, d.address()) };
                    ui.label(RichText::new(sub).color(col(color)));
                });
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let h = egui::vec2(0.0, 30.0);
                do_connect = ui.add(filled_button("Connect", theme::CONNECT).min_size(egui::vec2(110.0, 30.0))).clicked();
                do_save = ui.add(filled_button("Save", theme::ACCENT).min_size(egui::vec2(70.0, 30.0))).clicked();
                if self.confirm_delete {
                    do_delete = ui.add(filled_button("Really delete?", theme::DANGER).min_size(h)).clicked();
                    if ui.add(egui::Button::new("Cancel").min_size(h)).clicked() {
                        self.confirm_delete = false;
                    }
                } else if ui.add(outline_button("Delete", theme::DANGER).min_size(h)).clicked() {
                    self.confirm_delete = true;
                }
            });
            ui.add_space(12.0);
            card().show(ui, |ui| {
            // One fixed id for the grid: switching connections must not trigger a new sizing pass.
            egui::Grid::new("conn_form").num_columns(2).spacing([12.0, 8.0]).min_col_width(LABEL_WIDTH).show(ui, |ui| {
                let text = |ui: &mut egui::Ui, label: &str, value: &mut String, id: &str| {
                    ui.label(field_label(label));
                    ui.add(egui::TextEdit::singleline(value).desired_width(FIELD_WIDTH).id_salt(id));
                    ui.end_row();
                };
                text(ui, "Name", &mut d.name, "f_name");
                ui.label(field_label("Group"));
                let shown = if d.group.is_empty() { "(no group)".to_string() } else { d.group.clone() };
                egui::ComboBox::from_id_salt("f_group").width(FIELD_WIDTH - 8.0).selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.group, String::new(), "(no group)");
                    for g in &all_groups {
                        ui.selectable_value(&mut d.group, g.clone(), g);
                    }
                });
                ui.end_row();
                text(ui, "Host", &mut d.host, "f_host");
                ui.label(field_label("Port"));
                ui.add(egui::DragValue::new(&mut d.port).range(1..=65535));
                ui.end_row();
                text(ui, "Username", &mut d.username, "f_user");
                text(ui, "Domain", &mut d.domain, "f_domain");
                ui.label(field_label("Password"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut *d.password)
                            .password(!self.show_password)
                            .desired_width(FIELD_WIDTH - 56.0)
                            .id_salt("f_pw"),
                    );
                    if ui.add_sized([48.0, 20.0], egui::Button::new(if self.show_password { "Hide" } else { "Show" })).on_hover_text("Show / hide").clicked() {
                        self.show_password = !self.show_password;
                    }
                });
                ui.end_row();
                text(ui, "RD Gateway", &mut d.gateway, "f_gw");
                ui.label(field_label("Screen"));
                ui.horizontal(|ui| {
                    ui.checkbox(&mut d.fullscreen, "Full screen");
                    ui.add_enabled(!d.fullscreen, egui::DragValue::new(&mut d.width).range(640..=7680).suffix(" w"));
                    ui.add_enabled(!d.fullscreen, egui::DragValue::new(&mut d.height).range(480..=4320).suffix(" h"));
                });
                ui.end_row();
                ui.label(field_label("Options"));
                ui.vertical(|ui| {
                    ui.checkbox(&mut d.multimon, "Use all monitors");
                    ui.checkbox(&mut d.redirect_clipboard, "Share clipboard");
                    ui.checkbox(&mut d.redirect_drives, "Share local drives");
                    ui.checkbox(&mut d.admin_session, "Admin / console session");
                });
                ui.end_row();
                ui.label(field_label("Notes"));
                ui.add(egui::TextEdit::multiline(&mut d.notes).desired_width(FIELD_WIDTH).desired_rows(3).id_salt("f_notes"));
                ui.end_row();
            });
            });
        });

        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            do_save = true;
        }
        if do_save {
            self.save();
        }
        if do_connect {
            let c = self.draft.clone().unwrap();
            self.save();
            self.connect(&c);
        }
        if do_delete {
            if let Some(id) = self.selected.take() {
                self.vault.data.remove(id);
                self.draft = None;
                self.confirm_delete = false;
                self.save();
                self.status = "Deleted".into();
            }
        }
    }

    fn import(&mut self) {
        let Some(files) = rfd::FileDialog::new()
            .add_filter("Remote Desktop (.rdp, .rdm, .rdcx)", import::IMPORT_EXTENSIONS)
            .pick_files()
        else {
            return;
        };
        if import::needs_password(&files) {
            self.import_pw_dialog = Some(ImportPwDialog { files, password: String::new(), error: String::new() });
        } else {
            self.run_import(&files, None);
        }
    }

    /// Returns false when an export password was wrong, so the dialog can stay open.
    fn run_import(&mut self, files: &[PathBuf], password: Option<&str>) -> bool {
        let report = import::import_files(&mut self.vault.data, files, password);
        if report.added == 0 && report.errors.iter().any(|e| e.contains("Wrong export password")) {
            return false;
        }
        self.save_vault(&report.message());
        self.status = report.message();
        if let Some(id) = report.last_id {
            self.select(id);
        }
        true
    }

    fn import_pw_dialog_ui(&mut self, ui: &mut egui::Ui) {
        let Some(dlg) = self.import_pw_dialog.as_mut() else { return };
        let mut ok = false;
        let mut close = false;
        egui::Modal::new(egui::Id::unique("import_pw")).show(ui.ctx(), |ui| {
            ui.set_width(340.0);
            ui.heading("Import export file");
            ui.label(RichText::new("Enter the password that was chosen when the file was exported.").weak());
            ui.add_space(8.0);
            let r = ui.add(egui::TextEdit::singleline(&mut dlg.password).password(true).hint_text("Export password").desired_width(f32::INFINITY).id_salt("import_pw_field"));
            if !r.has_focus() && dlg.password.is_empty() {
                r.request_focus();
            }
            ok |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_sized([320.0, 18.0], egui::Label::new(RichText::new(&dlg.error).color(col(theme::DANGER))));
            ui.horizontal(|ui| {
                ok |= ui.button("Import").clicked();
                close = ui.button("Cancel").clicked();
            });
        });
        if ok {
            let files = dlg.files.clone();
            let mut pw = std::mem::take(&mut dlg.password);
            if self.run_import(&files, Some(&pw)) {
                close = true;
            } else if let Some(d) = self.import_pw_dialog.as_mut() {
                d.error = "Wrong export password".into();
            }
            wipe(&mut pw);
        }
        if close {
            if let Some(mut d) = self.import_pw_dialog.take() {
                wipe(&mut d.password);
            }
        }
    }

    fn open_export(&mut self, group: Option<String>) {
        self.export_dialog = Some(ExportDialog { group, password: String::new(), confirm: String::new(), move_after: false, error: String::new() });
    }

    fn export_dialog_ui(&mut self, ui: &mut egui::Ui) {
        let Some(dlg) = self.export_dialog.as_mut() else { return };
        let all_groups = self.vault.data.all_groups();
        let count = export::select(&self.vault.data, dlg.group.as_deref()).connections.len();
        let mut ok = false;
        let mut close = false;
        egui::Modal::new(egui::Id::unique("export_dialog")).show(ui.ctx(), |ui| {
            ui.set_width(380.0);
            ui.heading("Export connections");
            ui.label(RichText::new("Makes an encrypted .rdcx file with connections and passwords. Open it on the other computer with Import.").weak());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("What:");
                let shown = dlg.group.clone().unwrap_or_else(|| "All connections".into());
                egui::ComboBox::from_id_salt("export_scope").width(260.0).selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut dlg.group, None, "All connections");
                    for g in &all_groups {
                        ui.selectable_value(&mut dlg.group, Some(g.clone()), g);
                    }
                });
            });
            ui.label(RichText::new(format!("{count} connection(s)")).weak());
            ui.add_space(6.0);
            ui.add(egui::TextEdit::singleline(&mut dlg.password).password(true).hint_text("Password for the file").desired_width(f32::INFINITY).id_salt("export_pw"));
            ui.add(egui::TextEdit::singleline(&mut dlg.confirm).password(true).hint_text("Repeat password").desired_width(f32::INFINITY).id_salt("export_pw2"));
            ui.checkbox(&mut dlg.move_after, "Move: remove them from this computer after exporting");
            ui.add_sized([360.0, 18.0], egui::Label::new(RichText::new(&dlg.error).color(col(theme::DANGER))));
            ui.horizontal(|ui| {
                ok = ui.button("Export...").clicked();
                close = ui.button("Cancel").clicked();
            });
        });
        if ok {
            if count == 0 {
                dlg.error = "Nothing to export".into();
            } else if dlg.password.len() < 8 {
                dlg.error = "Use at least 8 characters".into();
            } else if dlg.password != dlg.confirm {
                dlg.error = "Passwords don't match".into();
            } else if let Some(dest) = rfd::FileDialog::new()
                .add_filter("myRDCcenter export", &[export::EXPORT_EXTENSION])
                .set_file_name("myRDCcenter-export.rdcx")
                .save_file()
            {
                match export::export_to(&self.vault.data, dlg.group.as_deref(), &dlg.password, &dest) {
                    Ok(n) => {
                        let group = dlg.group.clone();
                        let moved = dlg.move_after;
                        close = true;
                        if moved {
                            export::remove_exported(&mut self.vault.data, group.as_deref());
                            self.selected = None;
                            self.draft = None;
                            self.selected_group = None;
                            self.save_vault(&format!("Moved {n} connection(s) to {}", dest.display()));
                        } else {
                            self.status = format!("Exported {n} connection(s) to {}", dest.display());
                        }
                    }
                    Err(e) => dlg.error = e,
                }
            }
        }
        if close {
            if let Some(mut d) = self.export_dialog.take() {
                wipe(&mut d.password);
                wipe(&mut d.confirm);
            }
        }
    }

    fn backup(&mut self) {
        let Some(dest) = rfd::FileDialog::new().add_filter("myRDCcenter vault", &["rdsc"]).set_file_name("myrdccenter-backup.rdsc").save_file()
        else {
            return;
        };
        self.status = match self.vault.export_backup(&dest) {
            Ok(()) => format!("Encrypted backup saved to {}", dest.display()),
            Err(e) => e.to_string(),
        };
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        if !self.settings_open {
            return;
        }
        let mut close = false;
        let r = egui::Modal::new(egui::Id::unique("settings")).show(ui.ctx(), |ui| {
            ui.set_width(380.0);
            ui.heading("Settings");
            ui.add_space(8.0);
            ui.label(RichText::new("Vault").strong());
            ui.horizontal(|ui| {
                if ui.button("Backup...").clicked() {
                    close = true;
                    self.backup();
                }
                if ui.button("Change password").clicked() {
                    close = true;
                    self.change_pw = Some(Default::default());
                }
            });
            if keystore::SUPPORTED {
                let r = ui.checkbox(&mut self.remembered, "Remember on this computer").on_hover_text(keystore::label());
                if r.changed() {
                    self.status = match self.vault.set_remembered(self.remembered) {
                        Ok(()) if self.remembered => "The vault now opens with your computer login".into(),
                        Ok(()) => "The vault now asks for the master password".into(),
                        Err(e) => {
                            self.remembered = false;
                            e
                        }
                    };
                }
            }
            ui.horizontal(|ui| {
                ui.label("Auto-lock after (minutes, 0 = never):");
                let mut mins = self.vault.data.settings.auto_lock_minutes;
                if ui.add(egui::DragValue::new(&mut mins).range(0..=240)).changed() {
                    self.vault.data.settings.auto_lock_minutes = mins;
                    let _ = self.vault.save();
                }
            });
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if ui.add(filled_button("Close", theme::ACCENT)).clicked() {
                    close = true;
                }
            });
        });
        if close || r.should_close() {
            self.settings_open = false;
        }
    }

    fn change_password_ui(&mut self, ui: &mut egui::Ui) {
        let Some((new, confirm)) = self.change_pw.as_mut() else { return };
        let mut close = false;
        let mut apply = false;
        egui::Modal::new(egui::Id::unique("change_pw")).show(ui.ctx(), |ui| {
            ui.set_width(300.0);
            ui.heading("Change master password");
            ui.add_space(8.0);
            ui.add(egui::TextEdit::singleline(new).password(true).hint_text("New password").desired_width(f32::INFINITY));
            ui.add(egui::TextEdit::singleline(confirm).password(true).hint_text("Repeat").desired_width(f32::INFINITY));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                apply = ui.add(filled_button("Change", theme::ACCENT)).clicked();
                close = ui.button("Cancel").clicked();
            });
        });
        if apply {
            if new.len() < 8 {
                self.status = "Use at least 8 characters".into();
            } else if new != confirm {
                self.status = "Passwords don't match".into();
            } else {
                self.status = match self.vault.change_password(new) {
                    Ok(()) => "Master password changed".into(),
                    Err(e) => e.to_string(),
                };
                close = true;
            }
        }
        if close {
            if let Some((mut a, mut b)) = self.change_pw.take() {
                wipe(&mut a);
                wipe(&mut b);
            }
        }
    }
}

/// A small clickable expand/collapse triangle.
fn triangle(ui: &mut egui::Ui, expanded: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
    let color = if response.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
    let c = rect.center();
    let points = if expanded {
        vec![c + egui::vec2(-4.0, -2.0), c + egui::vec2(4.0, -2.0), c + egui::vec2(0.0, 3.0)]
    } else {
        vec![c + egui::vec2(-2.0, -4.0), c + egui::vec2(3.0, 0.0), c + egui::vec2(-2.0, 4.0)]
    };
    ui.painter().add(egui::Shape::convex_polygon(points, color, egui::Stroke::NONE));
    response
}

// ---------- Theme ----------

fn col(c: Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// `c` mixed with the window background; `amount` 0 = background, 1 = `c`.
fn tint(c: Rgb, amount: f32) -> Color32 {
    let mix = |a: u8, b: u8| (b as f32 + (a as f32 - b as f32) * amount).round() as u8;
    let bg = theme::BG;
    Color32::from_rgb(mix(c.0, bg.0), mix(c.1, bg.1), mix(c.2, bg.2))
}

/// Colourful light theme, shared with the Slint app through `myrdc_core::theme`.
fn apply_theme(style: &mut egui::Style) {
    use egui::{CornerRadius, Stroke};
    let v = &mut style.visuals;
    v.dark_mode = false;
    v.panel_fill = col(theme::BG);
    v.window_fill = col(theme::CARD);
    v.window_stroke = Stroke::new(1.0, col(theme::BORDER));
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.faint_bg_color = col(theme::CARD);
    v.extreme_bg_color = col(theme::CARD);
    v.text_edit_bg_color = Some(col(theme::CARD));
    v.code_bg_color = col(theme::BG);
    v.hyperlink_color = col(theme::ACCENT);
    v.error_fg_color = col(theme::DANGER);
    v.warn_fg_color = col(theme::GROUP_COLORS[2]);
    v.weak_text_color = Some(col(theme::TEXT_WEAK));
    v.selection.bg_fill = tint(theme::ACCENT, 0.16);
    v.selection.stroke = Stroke::new(1.0, col(theme::ACCENT));

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = col(theme::CARD);
    w.noninteractive.weak_bg_fill = col(theme::CARD);
    w.noninteractive.bg_stroke = Stroke::new(1.0, col(theme::BORDER));
    w.noninteractive.fg_stroke = Stroke::new(1.0, col(theme::TEXT));
    w.inactive.bg_fill = col(theme::BORDER);
    w.inactive.weak_bg_fill = col(theme::CARD);
    w.inactive.bg_stroke = Stroke::new(1.0, col(theme::BORDER));
    w.inactive.fg_stroke = Stroke::new(1.0, col(theme::TEXT));
    w.hovered.bg_fill = col(theme::HOVER);
    w.hovered.weak_bg_fill = col(theme::HOVER);
    w.hovered.bg_stroke = Stroke::new(1.0, tint(theme::ACCENT, 0.8));
    w.hovered.fg_stroke = Stroke::new(1.5, col(theme::TEXT));
    w.active.bg_fill = tint(theme::ACCENT, 0.7);
    w.active.weak_bg_fill = tint(theme::ACCENT, 0.7);
    w.active.bg_stroke = Stroke::new(1.0, col(theme::ACCENT));
    w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    w.open = w.hovered;
    for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.corner_radius = CornerRadius::same(6);
    }
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
}

/// Rounded panel used for forms and the lock screen.
fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(col(theme::CARD))
        .stroke(egui::Stroke::new(1.0, col(theme::BORDER)))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(16))
}

/// Button filled with a colour, white text.
fn filled_button(text: &str, color: Rgb) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).strong().color(Color32::WHITE)).fill(col(color)).stroke(egui::Stroke::NONE)
}

/// Button with a coloured border and text, for less important or risky actions.
fn outline_button(text: &str, color: Rgb) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(col(color)))
        .fill(tint(color, 0.07))
        .stroke(egui::Stroke::new(1.0, tint(color, 0.7)))
}

fn field_label(text: &str) -> RichText {
    RichText::new(text).color(col(theme::TEXT_WEAK))
}

fn dot(ui: &mut egui::Ui, color: Rgb, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, col(color));
}

/// Small rounded square in front of a group name.
fn swatch(ui: &mut egui::Ui, color: Rgb) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 3, col(color));
}

/// Number of connections in a group, as a coloured pill.
fn count_badge(ui: &mut egui::Ui, count: usize, color: Rgb) {
    let font = egui::FontId::proportional(11.0);
    let galley = ui.painter().layout_no_wrap(count.to_string(), font, col(color));
    let size = egui::vec2((galley.size().x + 12.0).max(22.0), 16.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, 8, tint(color, 0.16));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, col(color));
}

/// Round coloured badge with the first letter of a name.
fn avatar(ui: &mut egui::Ui, name: &str, color: Rgb) {
    let size = 44.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let p = ui.painter();
    p.circle_filled(rect.center(), size / 2.0, tint(color, 0.14));
    p.circle_stroke(rect.center(), size / 2.0 - 1.0, egui::Stroke::new(2.0, col(color)));
    let letter: String =
        name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
    p.text(rect.center(), egui::Align2::CENTER_CENTER, letter, egui::FontId::proportional(20.0), col(color));
}

/// App logo: a violet/cyan rounded square with a small "screen" inside.
fn logo(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let p = ui.painter();
    let r = (size * 0.25) as u8;
    p.rect_filled(rect, r, col(theme::ACCENT));
    let half = egui::Rect::from_min_max(rect.center(), rect.max);
    p.rect_filled(half, egui::CornerRadius { nw: 0, ne: 0, sw: 0, se: r }, col(theme::GROUP_COLORS[1]));
    let screen = rect.shrink(size * 0.24);
    p.rect_stroke(screen, (size * 0.08) as u8, egui::Stroke::new((size * 0.07).max(1.5), Color32::WHITE), egui::StrokeKind::Inside);
}

