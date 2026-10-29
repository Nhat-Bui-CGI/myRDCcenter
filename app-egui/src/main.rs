// RDS-center, egui front-end.
//
// Anti-flicker notes (egui can "blink" on Windows if left at defaults):
// - glow renderer with vsync, no MSAA, no transparency;
// - window stays hidden until the first frame is painted (eframe default) and
//   uses a fixed dark background so there is no white flash on start/resize;
// - no continuous repaint: we only repaint on input, plus one slow timer for auto-lock;
// - every container has a stable id so egui never has to re-measure (sizing pass)
//   when the selection changes, which is what makes grids flash for a frame;
// - the form keeps a fixed column width so the layout doesn't jump while typing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use rds_core::{default_vault_path, groups, rdp, Connection, TreeRow, Vault};
use uuid::Uuid;

const LABEL_WIDTH: f32 = 130.0;
const FIELD_WIDTH: f32 = 320.0;

fn main() -> eframe::Result {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RDS-center")
            .with_inner_size([1000.0, 640.0])
            .with_min_inner_size([720.0, 440.0])
            .with_transparent(false),
        renderer: eframe::Renderer::Glow,
        multisampling: 0,
        centered: true,
        dithering: false,
        ..Default::default()
    };
    options.glow_options.vsync = true;
    eframe::run_native("RDS-center", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

enum Screen {
    Locked(LockScreen),
    Open(Box<OpenVault>),
}

#[derive(Default)]
struct LockScreen {
    password: String,
    confirm: String,
    error: String,
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
    /// Group selected in the tree (when no connection is selected).
    selected_group: Option<String>,
    group_dialog: Option<GroupDialog>,
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
}

struct App {
    vault_path: PathBuf,
    screen: Screen,
    last_activity: Instant,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        cc.egui_ctx.all_styles_mut(|style| {
            // No fade animations: they are a common source of visible flicker.
            style.animation_time = 0.0;
        });
        Self { vault_path: default_vault_path(), screen: Screen::Locked(LockScreen::default()), last_activity: Instant::now() }
    }

    fn lock(&mut self) {
        self.screen = Screen::Locked(LockScreen::default());
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
        // Same as the dark panel fill, so a resize never shows a different colour.
        egui::Color32::from_gray(27).to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.auto_lock_check(&ctx);

        let mut lock_now = false;
        let mut unlocked = None;
        match &mut self.screen {
            Screen::Locked(lock) => unlocked = lock_ui(ui, lock, &self.vault_path),
            Screen::Open(open) => lock_now = open.ui(ui),
        }
        if let Some(vault) = unlocked {
            self.last_activity = Instant::now();
            self.screen = Screen::Open(Box::new(OpenVault::new(vault)));
        }
        if lock_now {
            self.lock();
        }
    }
}

fn lock_ui(ui: &mut egui::Ui, lock: &mut LockScreen, path: &PathBuf) -> Option<Vault> {
    let creating = !Vault::exists(path);
    let mut result = None;
    egui::CentralPanel::default().show(ui, |ui| {
        ui.add_space(ui.available_height() * 0.25);
        ui.vertical_centered(|ui| {
            ui.heading(RichText::new("RDS-center").size(28.0));
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
            ui.add_space(8.0);
            submit |= ui.button(if creating { "Create vault" } else { "Unlock" }).clicked();
            // Fixed-height error line, so the layout doesn't jump when an error shows up.
            ui.add_sized([400.0, 20.0], egui::Label::new(RichText::new(&lock.error).color(Color32::LIGHT_RED)));
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
                    Ok(v) => result = Some(v),
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
    result
}

fn wipe(s: &mut String) {
    // Overwrite before clearing so the password doesn't linger in memory.
    unsafe { s.as_bytes_mut().fill(0) };
    s.clear();
}

impl OpenVault {
    fn new(vault: Vault) -> Self {
        Self {
            vault,
            search: String::new(),
            selected: None,
            draft: None,
            show_password: false,
            status: String::new(),
            confirm_delete: false,
            change_pw: None,
            selected_group: None,
            group_dialog: None,
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
    fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut lock = false;

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("New connection").clicked() {
                    let group = self.current_group();
                    self.new_connection(&group);
                }
                if ui.button("New group").clicked() {
                    let group = self.current_group();
                    self.open_new_group(&group);
                }
                if ui.button("Import .rdp").clicked() {
                    self.import();
                }
                if ui.button("Backup").clicked() {
                    self.backup();
                }
                if ui.button("Change password").clicked() {
                    self.change_pw = Some(Default::default());
                }
                ui.separator();
                ui.label("Auto-lock (min):");
                let mut mins = self.vault.data.settings.auto_lock_minutes;
                if ui.add(egui::DragValue::new(&mut mins).range(0..=240)).changed() {
                    self.vault.data.settings.auto_lock_minutes = mins;
                    let _ = self.vault.save();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Lock").clicked() {
                        lock = true;
                    }
                });
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_sized([ui.available_width(), 18.0], egui::Label::new(RichText::new(&self.status).weak()).truncate());
        });

        egui::Panel::left("list").resizable(true).default_size(280.0).size_range(200.0..=500.0).show(ui, |ui| {
            ui.add_space(4.0);
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search").desired_width(f32::INFINITY).id_salt("search"));
            ui.add_space(4.0);
            self.list_ui(ui);
        });

        egui::CentralPanel::default().show(ui, |ui| self.detail_ui(ui));

        self.change_password_ui(ui);
        self.group_dialog_ui(ui);
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
        let all_groups = self.vault.data.all_groups();
        egui::ScrollArea::vertical().id_salt("list_scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for row in &rows {
                    match row {
                        TreeRow::Group { path, name, depth, expanded, count } => {
                            let selected = self.selected_group.as_deref() == Some(path.as_str());
                            ui.horizontal(|ui| {
                                ui.add_space(*depth as f32 * 16.0);
                                if triangle(ui, *expanded).clicked() {
                                    action = Some(TreeAction::ToggleGroup(path.clone()));
                                }
                                let label = RichText::new(format!("{name}  ({count})")).strong();
                                let r = ui.add(egui::Button::selectable(selected, label));
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
        ui.heading(groups::leaf(&path));
        ui.label(RichText::new(&path).weak());
        ui.add_space(4.0);
        ui.label(format!("{count} connection(s) in this group and its subgroups."));
        ui.add_space(12.0);
        let mut action = None;
        ui.horizontal(|ui| {
            if ui.button("New connection here").clicked() {
                action = Some(TreeAction::NewConnectionIn(path.clone()));
            }
            if ui.button("New subgroup").clicked() {
                action = Some(TreeAction::NewGroupIn(path.clone()));
            }
            if ui.button("Rename / move").clicked() {
                action = Some(TreeAction::RenameGroup(path.clone()));
            }
            if self.confirm_delete_group {
                if ui.button(RichText::new("Really delete group?").color(Color32::LIGHT_RED)).clicked() {
                    self.vault.data.delete_group(&path);
                    self.selected_group = None;
                    self.confirm_delete_group = false;
                    self.save_vault("Group deleted. Its connections moved up one level.");
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_delete_group = false;
                }
            } else if ui.button("Delete group").clicked() {
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
            ui.add_sized([320.0, 18.0], egui::Label::new(RichText::new(&dlg.error).color(Color32::LIGHT_RED)));
            ui.horizontal(|ui| {
                apply |= ui.button("OK").clicked();
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
                ui.centered_and_justified(|ui| ui.label(RichText::new("Select a connection, or double-click to connect").weak()));
            }
            return;
        }
        let all_groups = self.vault.data.all_groups();
        let Some(d) = self.draft.as_mut() else { return };
        let mut do_save = false;
        let mut do_connect = false;
        let mut do_delete = false;

        egui::ScrollArea::vertical().id_salt("detail_scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.heading(d.display_name().to_string());
            ui.add_space(8.0);
            // One fixed id for the grid: switching connections must not trigger a new sizing pass.
            egui::Grid::new("conn_form").num_columns(2).spacing([12.0, 8.0]).min_col_width(LABEL_WIDTH).show(ui, |ui| {
                let text = |ui: &mut egui::Ui, label: &str, value: &mut String, id: &str| {
                    ui.label(label);
                    ui.add(egui::TextEdit::singleline(value).desired_width(FIELD_WIDTH).id_salt(id));
                    ui.end_row();
                };
                text(ui, "Name", &mut d.name, "f_name");
                ui.label("Group");
                let shown = if d.group.is_empty() { "(no group)".to_string() } else { d.group.clone() };
                egui::ComboBox::from_id_salt("f_group").width(FIELD_WIDTH - 8.0).selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.group, String::new(), "(no group)");
                    for g in &all_groups {
                        ui.selectable_value(&mut d.group, g.clone(), g);
                    }
                });
                ui.end_row();
                text(ui, "Host", &mut d.host, "f_host");
                ui.label("Port");
                ui.add(egui::DragValue::new(&mut d.port).range(1..=65535));
                ui.end_row();
                text(ui, "Username", &mut d.username, "f_user");
                text(ui, "Domain", &mut d.domain, "f_domain");
                ui.label("Password");
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
                ui.label("Screen");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut d.fullscreen, "Full screen");
                    ui.add_enabled(!d.fullscreen, egui::DragValue::new(&mut d.width).range(640..=7680).suffix(" w"));
                    ui.add_enabled(!d.fullscreen, egui::DragValue::new(&mut d.height).range(480..=4320).suffix(" h"));
                });
                ui.end_row();
                ui.label("Options");
                ui.vertical(|ui| {
                    ui.checkbox(&mut d.multimon, "Use all monitors");
                    ui.checkbox(&mut d.redirect_clipboard, "Share clipboard");
                    ui.checkbox(&mut d.redirect_drives, "Share local drives");
                    ui.checkbox(&mut d.admin_session, "Admin / console session");
                });
                ui.end_row();
                ui.label("Notes");
                ui.add(egui::TextEdit::multiline(&mut d.notes).desired_width(FIELD_WIDTH).desired_rows(3).id_salt("f_notes"));
                ui.end_row();
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                do_connect = ui.add(egui::Button::new(RichText::new("Connect").strong())).clicked();
                do_save = ui.button("Save").clicked();
                if self.confirm_delete {
                    do_delete = ui.button(RichText::new("Really delete?").color(Color32::LIGHT_RED)).clicked();
                    if ui.button("Cancel").clicked() {
                        self.confirm_delete = false;
                    }
                } else if ui.button("Delete").clicked() {
                    self.confirm_delete = true;
                }
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
        let Some(files) = rfd::FileDialog::new().add_filter("Remote Desktop", &["rdp"]).pick_files() else { return };
        let mut n = 0;
        let mut last = None;
        for f in files {
            match rdp::import_rdp(&f) {
                Ok(c) => {
                    last = Some(c.id);
                    self.vault.data.connections.push(c);
                    n += 1;
                }
                Err(e) => self.status = format!("{}: {e}", f.display()),
            }
        }
        if n > 0 {
            self.save();
            self.status = format!("Imported {n} connection(s) into \"Imported\"");
            if let Some(id) = last {
                self.select(id);
            }
        }
    }

    fn backup(&mut self) {
        let Some(dest) = rfd::FileDialog::new().add_filter("RDS-center vault", &["rdsc"]).set_file_name("rds-center-backup.rdsc").save_file()
        else {
            return;
        };
        self.status = match self.vault.export_backup(&dest) {
            Ok(()) => format!("Encrypted backup saved to {}", dest.display()),
            Err(e) => e.to_string(),
        };
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
                apply = ui.button("Change").clicked();
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
