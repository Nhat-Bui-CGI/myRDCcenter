// RDS-center, Slint front-end.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rds_core::{default_vault_path, groups, idle, import, rdp, Connection, Secret, TreeRow, Vault};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use uuid::Uuid;

slint::include_modules!();

struct State {
    path: PathBuf,
    vault: Option<Vault>,
    selected: Option<Uuid>,
    selected_group: Option<String>,
    /// Group dialog: Some(path) = renaming that group, None = new group under `dialog_parent`.
    dialog_renaming: Option<String>,
    dialog_parent: String,
    last_activity: Instant,
}

const NO_GROUP: &str = "(no group)";

type Shared = Rc<RefCell<State>>;

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let path = default_vault_path();
    ui.set_vault_path(path.display().to_string().into());
    ui.set_creating(!Vault::exists(&path));
    let state: Shared = Rc::new(RefCell::new(State { path, vault: None, selected: None, selected_group: None, dialog_renaming: None, dialog_parent: String::new(), last_activity: Instant::now() }));

    // Every callback counts as activity for auto-lock (used when system idle time isn't available).
    let touch = {
        let s = state.clone();
        move || s.borrow_mut().last_activity = Instant::now()
    };

    ui.on_unlock({
        let (ui_w, s) = (ui.as_weak(), state.clone());
        move |pw, confirm| {
            let ui = ui_w.unwrap();
            let mut st = s.borrow_mut();
            let creating = !Vault::exists(&st.path);
            let res = if creating {
                if pw.len() < 8 {
                    Err("Use at least 8 characters".to_string())
                } else if pw != confirm {
                    Err("Passwords don't match".to_string())
                } else {
                    Vault::create(&st.path, &pw).map_err(|e| e.to_string())
                }
            } else {
                Vault::open(&st.path, &pw).map_err(|e| e.to_string())
            };
            match res {
                Ok(v) => {
                    ui.set_auto_lock_minutes(v.data.settings.auto_lock_minutes as i32);
                    st.vault = Some(v);
                    st.selected = None;
                    st.last_activity = Instant::now();
                    drop(st);
                    ui.set_lock_error("".into());
                    ui.set_search("".into());
                    ui.set_has_draft(false);
                    ui.set_status("".into());
                    ui.set_locked(false);
                    refresh_list(&ui, &s);
                }
                Err(e) => ui.set_lock_error(e.into()),
            }
        }
    });

    ui.on_search_changed({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move |_| {
            t();
            refresh_list(&ui_w.unwrap(), &s);
        }
    });

    ui.on_select({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move |id| {
            t();
            select(&ui_w.unwrap(), &s, &id);
        }
    });

    ui.on_activate({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move |id| {
            t();
            let ui = ui_w.unwrap();
            select(&ui, &s, &id);
            let conn = Uuid::parse_str(&id).ok().and_then(|id| s.borrow().vault.as_ref()?.data.find(id).cloned());
            if let Some(c) = conn {
                launch(&ui, &c);
            }
        }
    });

    ui.on_new_connection({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move || {
            t();
            let ui = ui_w.unwrap();
            let group = current_group(&ui, &s);
            new_connection(&ui, &s, &group);
        }
    });

    ui.on_new_connection_here({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move || {
            t();
            let ui = ui_w.unwrap();
            let group = s.borrow().selected_group.clone().unwrap_or_default();
            new_connection(&ui, &s, &group);
        }
    });

    ui.on_click_group({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move |path| {
            t();
            let ui = ui_w.unwrap();
            let path = path.to_string();
            let already = s.borrow().selected_group.as_deref() == Some(path.as_str());
            if let Some(v) = s.borrow_mut().vault.as_mut() {
                // First click selects (and opens) a group, a second click folds it.
                let fold = already && !v.data.is_collapsed(&path);
                v.data.set_collapsed(&path, fold);
                let _ = v.save();
            }
            select_group(&ui, &s, &path);
        }
    });

    ui.on_toggle_group({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move |path| {
            t();
            let ui = ui_w.unwrap();
            if let Some(v) = s.borrow_mut().vault.as_mut() {
                let collapsed = v.data.is_collapsed(&path);
                v.data.set_collapsed(&path, !collapsed);
                let _ = v.save();
            }
            refresh_list(&ui, &s);
        }
    });

    ui.on_new_group({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move || {
            t();
            let ui = ui_w.unwrap();
            let parent = current_group(&ui, &s);
            let mut st = s.borrow_mut();
            st.dialog_renaming = None;
            st.dialog_parent = parent.clone();
            drop(st);
            ui.set_group_dialog_title("New group".into());
            ui.set_group_dialog_hint(if parent.is_empty() { "".into() } else { format!("Inside {parent}").into() });
            ui.set_group_dialog_name("".into());
            ui.set_group_dialog_error("".into());
            ui.set_group_dialog_open(true);
        }
    });

    ui.on_rename_group({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move || {
            t();
            let ui = ui_w.unwrap();
            let Some(path) = s.borrow().selected_group.clone() else { return };
            s.borrow_mut().dialog_renaming = Some(path.clone());
            ui.set_group_dialog_title("Rename group".into());
            ui.set_group_dialog_hint(format!("Use / to move it, e.g. Work/{}", groups::leaf(&path)).into());
            ui.set_group_dialog_name(path.into());
            ui.set_group_dialog_error("".into());
            ui.set_group_dialog_open(true);
        }
    });

    ui.on_group_dialog_ok({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move |name| {
            t();
            let ui = ui_w.unwrap();
            let name = groups::normalize(&name);
            if name.is_empty() {
                ui.set_group_dialog_error("Enter a name".into());
                return;
            }
            let mut st = s.borrow_mut();
            let renaming = st.dialog_renaming.clone();
            let parent = st.dialog_parent.clone();
            let Some(v) = st.vault.as_mut() else { return };
            let result = match renaming {
                Some(old) => v.data.rename_group(&old, &name).map(|_| (name.clone(), "Group renamed")),
                None => {
                    let full = if parent.is_empty() { name.clone() } else { format!("{parent}/{name}") };
                    v.data.set_collapsed(&parent, false);
                    Ok((v.data.add_group(&full), "Group created"))
                }
            };
            match result {
                Ok((path, msg)) => {
                    let msg = match v.save() {
                        Ok(()) => msg.to_string(),
                        Err(e) => e.to_string(),
                    };
                    drop(st);
                    ui.set_status(msg.into());
                    ui.set_group_dialog_open(false);
                    select_group(&ui, &s, &path);
                }
                Err(e) => ui.set_group_dialog_error(e.into()),
            }
        }
    });

    ui.on_delete_group({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move || {
            t();
            let ui = ui_w.unwrap();
            let mut st = s.borrow_mut();
            let Some(path) = st.selected_group.take() else { return };
            if let Some(v) = st.vault.as_mut() {
                v.data.delete_group(&path);
                let msg = match v.save() {
                    Ok(()) => "Group deleted. Its connections moved up one level.".to_string(),
                    Err(e) => e.to_string(),
                };
                ui.set_status(msg.into());
            }
            drop(st);
            ui.set_selected_group("".into());
            ui.set_confirm_delete_group(false);
            refresh_list(&ui, &s);
        }
    });

    ui.on_save({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move || {
            t();
            let ui = ui_w.unwrap();
            save_draft(&ui, &s);
        }
    });

    ui.on_connect({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move || {
            t();
            let ui = ui_w.unwrap();
            if let Some(c) = save_draft(&ui, &s) {
                launch(&ui, &c);
            }
        }
    });

    ui.on_delete({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move || {
            t();
            let ui = ui_w.unwrap();
            let mut st = s.borrow_mut();
            if let (Some(id), Some(v)) = (st.selected, st.vault.as_mut()) {
                v.data.remove(id);
                let msg = match v.save() {
                    Ok(()) => "Deleted".to_string(),
                    Err(e) => e.to_string(),
                };
                ui.set_status(msg.into());
            }
            st.selected = None;
            drop(st);
            ui.set_has_draft(false);
            ui.set_confirm_delete(false);
            refresh_list(&ui, &s);
        }
    });

    ui.on_import_rdp({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move || {
            t();
            let ui = ui_w.unwrap();
            let Some(files) = rfd::FileDialog::new()
                .add_filter("Remote Desktop (.rdp, .rdm)", import::IMPORT_EXTENSIONS)
                .pick_files()
            else {
                return;
            };
            let mut st = s.borrow_mut();
            let Some(v) = st.vault.as_mut() else { return };
            let report = import::import_files(&mut v.data, &files);
            let msg = match v.save() {
                Ok(()) => report.message(),
                Err(e) => e.to_string(),
            };
            ui.set_status(msg.into());
            let last = report.last_id.and_then(|id| v.data.find(id).cloned());
            if let Some(c) = last {
                st.selected = Some(c.id);
                st.selected_group = None;
                ui.set_selected_group("".into());
                show_draft(&ui, &c);
            }
            drop(st);
            refresh_list(&ui, &s);
        }
    });

    ui.on_backup({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move || {
            t();
            let ui = ui_w.unwrap();
            let Some(dest) = rfd::FileDialog::new()
                .add_filter("RDS-center vault", &["rdsc"])
                .set_file_name("rds-center-backup.rdsc")
                .save_file()
            else {
                return;
            };
            let st = s.borrow();
            if let Some(v) = st.vault.as_ref() {
                let msg = match v.export_backup(&dest) {
                    Ok(()) => format!("Encrypted backup saved to {}", dest.display()),
                    Err(e) => e.to_string(),
                };
                ui.set_status(msg.into());
            }
        }
    });

    ui.on_change_password({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move |new, confirm| {
            t();
            let ui = ui_w.unwrap();
            if new.len() < 8 {
                ui.set_status("Use at least 8 characters".into());
                return;
            }
            if new != confirm {
                ui.set_status("Passwords don't match".into());
                return;
            }
            if let Some(v) = s.borrow_mut().vault.as_mut() {
                let msg = match v.change_password(&new) {
                    Ok(()) => "Master password changed".to_string(),
                    Err(e) => e.to_string(),
                };
                ui.set_status(msg.into());
            }
            ui.set_change_pw_open(false);
        }
    });

    ui.on_auto_lock_changed({
        let (s, t) = (state.clone(), touch.clone());
        move |mins| {
            t();
            if let Some(v) = s.borrow_mut().vault.as_mut() {
                v.data.settings.auto_lock_minutes = mins.max(0) as u32;
                let _ = v.save();
            }
        }
    });

    ui.on_lock({
        let (ui_w, s) = (ui.as_weak(), state.clone());
        move || lock(&ui_w.unwrap(), &s)
    });

    // Auto-lock check every 5 seconds.
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(5), {
        let (ui_w, s) = (ui.as_weak(), state.clone());
        move || {
            let Some(ui) = ui_w.upgrade() else { return };
            let st = s.borrow();
            let Some(v) = st.vault.as_ref() else { return };
            let mins = v.data.settings.auto_lock_minutes as u64;
            if mins == 0 {
                return;
            }
            let idle = idle::system_idle_secs().unwrap_or_else(|| st.last_activity.elapsed().as_secs());
            if idle >= mins * 60 {
                drop(st);
                lock(&ui, &s);
            }
        }
    });

    ui.run()
}

fn lock(ui: &MainWindow, s: &Shared) {
    let mut st = s.borrow_mut();
    st.vault = None; // drops and wipes the key
    st.selected = None;
    st.selected_group = None;
    ui.set_creating(!Vault::exists(&st.path));
    drop(st);
    // Clear everything sensitive from the UI.
    ui.set_draft(ConnData::default());
    ui.set_has_draft(false);
    ui.set_rows(ModelRc::default());
    ui.set_change_pw_open(false);
    ui.set_group_dialog_open(false);
    ui.set_selected_group("".into());
    ui.set_show_password(false);
    ui.set_locked(true);
}

/// Group that new items go into: the selected group, or the open connection's group.
fn current_group(ui: &MainWindow, s: &Shared) -> String {
    if let Some(g) = s.borrow().selected_group.clone() {
        return g;
    }
    if ui.get_has_draft() {
        let g = ui.get_draft().group.to_string();
        if g != NO_GROUP {
            return groups::normalize(&g);
        }
    }
    String::new()
}

fn new_connection(ui: &MainWindow, s: &Shared, group: &str) {
    let c = Connection::new("New connection", group);
    let mut st = s.borrow_mut();
    st.selected = Some(c.id);
    st.selected_group = None;
    drop(st);
    ui.set_selected_group("".into());
    show_draft(ui, &c);
    refresh_list(ui, s);
}

fn select_group(ui: &MainWindow, s: &Shared, path: &str) {
    let mut st = s.borrow_mut();
    st.selected_group = Some(path.to_string());
    st.selected = None;
    let count = st.vault.as_ref().map(|v| v.data.count_in(path)).unwrap_or(0);
    drop(st);
    ui.set_has_draft(false);
    ui.set_draft(ConnData::default());
    ui.set_selected_id("".into());
    ui.set_selected_group(path.into());
    ui.set_group_count_text(format!("{count} connection(s) in this group and its subgroups.").into());
    ui.set_confirm_delete_group(false);
    refresh_list(ui, s);
}

fn select(ui: &MainWindow, s: &Shared, id: &str) {
    let Ok(id) = Uuid::parse_str(id) else { return };
    let conn = s.borrow().vault.as_ref().and_then(|v| v.data.find(id).cloned());
    if let Some(c) = conn {
        let mut st = s.borrow_mut();
        st.selected = Some(id);
        st.selected_group = None;
        drop(st);
        ui.set_selected_group("".into());
        show_draft(ui, &c);
        ui.set_selected_id(id.to_string().into());
    }
}

fn show_draft(ui: &MainWindow, c: &Connection) {
    ui.set_draft(to_data(c));
    ui.set_has_draft(true);
    ui.set_show_password(false);
    ui.set_confirm_delete(false);
    ui.set_selected_id(c.id.to_string().into());
}

/// Write the form back into the vault and save. Returns the saved connection.
fn save_draft(ui: &MainWindow, s: &Shared) -> Option<Connection> {
    if !ui.get_has_draft() {
        return None;
    }
    let c = from_data(&ui.get_draft())?;
    let mut st = s.borrow_mut();
    let v = st.vault.as_mut()?;
    match v.data.find_mut(c.id) {
        Some(existing) => *existing = c.clone(),
        None => v.data.connections.push(c.clone()),
    }
    let msg = match v.save() {
        Ok(()) => "Saved".to_string(),
        Err(e) => e.to_string(),
    };
    drop(st);
    ui.set_status(msg.into());
    refresh_list(ui, s);
    Some(c)
}

fn launch(ui: &MainWindow, c: &Connection) {
    let msg = match rdp::launch(c) {
        Ok(()) => format!("Connecting to {}…", c.display_name()),
        Err(e) => e,
    };
    ui.set_status(msg.into());
}

fn refresh_list(ui: &MainWindow, s: &Shared) {
    let st = s.borrow();
    let Some(v) = st.vault.as_ref() else { return };
    let rows: Vec<ListRow> = v
        .data
        .tree_rows(&ui.get_search())
        .into_iter()
        .map(|r| match r {
            TreeRow::Group { path, name, depth, expanded, count } => ListRow {
                id: "".into(),
                group_path: path.into(),
                title: format!("{name}  ({count})").into(),
                tooltip: "".into(),
                depth: depth as i32,
                expanded,
            },
            TreeRow::Connection { id, name, address, depth } => ListRow {
                id: id.to_string().into(),
                group_path: "".into(),
                title: name.into(),
                tooltip: address.into(),
                depth: depth as i32,
                expanded: false,
            },
        })
        .collect();
    let mut names: Vec<SharedString> = vec![NO_GROUP.into()];
    names.extend(v.data.all_groups().into_iter().map(SharedString::from));
    ui.set_group_names(ModelRc::new(VecModel::from(names)));
    ui.set_rows(ModelRc::new(VecModel::from(rows)));
}

fn to_data(c: &Connection) -> ConnData {
    ConnData {
        id: c.id.to_string().into(),
        name: c.name.clone().into(),
        group: if c.group.is_empty() { NO_GROUP.into() } else { c.group.clone().into() },
        host: c.host.clone().into(),
        port: c.port as i32,
        username: c.username.clone().into(),
        domain: c.domain.clone().into(),
        password: c.password.as_str().into(),
        gateway: c.gateway.clone().into(),
        fullscreen: c.fullscreen,
        width: c.width as i32,
        height: c.height as i32,
        multimon: c.multimon,
        redirect_clipboard: c.redirect_clipboard,
        redirect_drives: c.redirect_drives,
        admin_session: c.admin_session,
        notes: c.notes.clone().into(),
    }
}

fn from_data(d: &ConnData) -> Option<Connection> {
    let s = |v: &SharedString| v.to_string();
    Some(Connection {
        id: Uuid::parse_str(&d.id).ok()?,
        name: s(&d.name),
        group: if d.group == NO_GROUP { String::new() } else { groups::normalize(&d.group) },
        host: s(&d.host).trim().to_string(),
        port: d.port.clamp(1, 65535) as u16,
        username: s(&d.username),
        domain: s(&d.domain),
        password: Secret::new(s(&d.password)),
        gateway: s(&d.gateway),
        fullscreen: d.fullscreen,
        width: d.width.max(640) as u32,
        height: d.height.max(480) as u32,
        multimon: d.multimon,
        redirect_clipboard: d.redirect_clipboard,
        redirect_drives: d.redirect_drives,
        admin_session: d.admin_session,
        notes: s(&d.notes),
    })
}
