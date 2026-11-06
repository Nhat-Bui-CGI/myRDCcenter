// myRDCcenter, Slint front-end.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use myrdc_core::prefs::{Prefs, TextSizeKey};
use myrdc_core::theme;
use myrdc_core::{default_vault_path, export, groups, idle, import, keystore, rdp, Connection, Secret, TreeRow, Vault};
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
    /// .rdcx files waiting for their export password.
    pending_import: Vec<PathBuf>,
    last_activity: Instant,
}

const NO_GROUP: &str = "(no group)";

type Shared = Rc<RefCell<State>>;

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    ui.on_group_color(|g| {
        let g = if g == NO_GROUP { "" } else { g.as_str() };
        to_color(theme::group_color(g))
    });
    ui.on_status_color(|s| to_color(theme::status_color(&s)));
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    text_size_setup(&ui);
    let path = default_vault_path();
    ui.set_vault_path(path.display().to_string().into());
    ui.set_creating(!Vault::exists(&path));
    ui.set_remember_supported(keystore::SUPPORTED);
    ui.set_remember_label(keystore::label().into());
    ui.set_has_remembered(keystore::load(&path).is_some());
    let state: Shared = Rc::new(RefCell::new(State { path, vault: None, selected: None, selected_group: None, dialog_renaming: None, dialog_parent: String::new(), pending_import: Vec::new(), last_activity: Instant::now() }));

    // Every callback counts as activity for auto-lock (used when system idle time isn't available).
    let touch = {
        let s = state.clone();
        move || s.borrow_mut().last_activity = Instant::now()
    };

    ui.on_unlock({
        let (ui_w, s) = (ui.as_weak(), state.clone());
        move |pw, confirm, remember| {
            let ui = ui_w.unwrap();
            let st = s.borrow_mut();
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
            let res = res.and_then(|v| {
                if remember {
                    v.set_remembered(true)?;
                }
                Ok(v)
            });
            match res {
                Ok(v) => {
                    drop(st);
                    enter_vault(&ui, &s, v);
                }
                Err(e) => ui.set_lock_error(e.into()),
            }
        }
    });

    ui.on_unlock_remembered({
        let (ui_w, s) = (ui.as_weak(), state.clone());
        move || {
            let ui = ui_w.unwrap();
            let path = s.borrow().path.clone();
            match Vault::open_remembered(&path) {
                Some(Ok(v)) => enter_vault(&ui, &s, v),
                Some(Err(e)) => ui.set_lock_error(e.to_string().into()),
                None => {
                    ui.set_has_remembered(false);
                    ui.set_lock_error("No remembered key found. Use the master password.".into());
                }
            }
        }
    });

    ui.on_set_remember({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move |on| {
            t();
            let ui = ui_w.unwrap();
            let st = s.borrow();
            let Some(v) = st.vault.as_ref() else { return };
            let msg = match v.set_remembered(on) {
                Ok(()) if on => "The vault now opens with your computer login".to_string(),
                Ok(()) => "The vault now asks for the master password".to_string(),
                Err(e) => {
                    ui.set_remembered(false);
                    e
                }
            };
            ui.set_status(msg.into());
        }
    });

    // Open straight away when the key is remembered on this computer.
    let startup_path = state.borrow().path.clone();
    if let Some(Ok(v)) = Vault::open_remembered(&startup_path) {
        enter_vault(&ui, &state, v);
    }
    #[cfg(feature = "screenshot")]
    let _shot_timer = screenshot(&ui, &state);

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
    ui.set_export_open(false);
    ui.set_import_pw_open(false);
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
                .add_filter("Remote Desktop (.rdp, .rdm, .rdcx)", import::IMPORT_EXTENSIONS)
                .pick_files()
            else {
                return;
            };
            if import::needs_password(&files) {
                s.borrow_mut().pending_import = files;
                ui.set_import_pw_error("".into());
                ui.set_import_pw_open(true);
            } else {
                run_import(&ui, &s, &files, None);
            }
        }
    });

    ui.on_import_pw_ok({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move |pw| {
            t();
            let ui = ui_w.unwrap();
            let files = s.borrow().pending_import.clone();
            if run_import(&ui, &s, &files, Some(&pw)) {
                s.borrow_mut().pending_import.clear();
                ui.set_import_pw_open(false);
            } else {
                ui.set_import_pw_error("Wrong export password".into());
            }
        }
    });

    ui.on_export_request({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move |group| {
            t();
            let ui = ui_w.unwrap();
            let st = s.borrow();
            let Some(v) = st.vault.as_ref() else { return };
            let mut scopes: Vec<SharedString> = vec![ALL_CONNECTIONS.into()];
            scopes.extend(v.data.all_groups().into_iter().map(SharedString::from));
            drop(st);
            ui.set_export_scopes(ModelRc::new(VecModel::from(scopes)));
            let scope: SharedString = if group.is_empty() { ALL_CONNECTIONS.into() } else { group };
            ui.set_export_scope(scope.clone());
            update_export_count(&ui, &s, &scope);
            ui.set_export_error("".into());
            ui.set_export_move(false);
            ui.set_export_open(true);
        }
    });

    ui.on_export_scope_changed({
        let (ui_w, s) = (ui.as_weak(), state.clone());
        move |scope| update_export_count(&ui_w.unwrap(), &s, &scope)
    });

    ui.on_export_ok({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move |pw, confirm| {
            t();
            let ui = ui_w.unwrap();
            if pw.len() < 8 {
                ui.set_export_error("Use at least 8 characters".into());
                return;
            }
            if pw != confirm {
                ui.set_export_error("Passwords don't match".into());
                return;
            }
            let scope = ui.get_export_scope();
            let group = (scope != ALL_CONNECTIONS).then(|| scope.to_string());
            let Some(dest) = rfd::FileDialog::new()
                .add_filter("myRDCcenter export", &[export::EXPORT_EXTENSION])
                .set_file_name("myRDCcenter-export.rdcx")
                .save_file()
            else {
                return;
            };
            let mut st = s.borrow_mut();
            let Some(v) = st.vault.as_mut() else { return };
            match export::export_to(&v.data, group.as_deref(), &pw, &dest) {
                Ok(n) => {
                    let msg = if ui.get_export_move() {
                        export::remove_exported(&mut v.data, group.as_deref());
                        match v.save() {
                            Ok(()) => format!("Moved {n} connection(s) to {}", dest.display()),
                            Err(e) => e.to_string(),
                        }
                    } else {
                        format!("Exported {n} connection(s) to {}", dest.display())
                    };
                    let moved = ui.get_export_move();
                    if moved {
                        st.selected = None;
                        st.selected_group = None;
                    }
                    drop(st);
                    if moved {
                        ui.set_has_draft(false);
                        ui.set_selected_group("".into());
                    }
                    ui.set_status(msg.into());
                    ui.set_export_open(false);
                    refresh_list(&ui, &s);
                }
                Err(e) => ui.set_export_error(e.into()),
            }
        }
    });

    ui.on_backup({
        let (ui_w, s, t) = (ui.as_weak(), state.clone(), touch.clone());
        move || {
            t();
            let ui = ui_w.unwrap();
            let Some(dest) = rfd::FileDialog::new()
                .add_filter("myRDCcenter vault", &["rdsc"])
                .set_file_name("myrdccenter-backup.rdsc")
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

/// Text size: the whole UI is drawn at (display scale x text size).
///
/// Slint has no zoom of its own, so the app sets the window scale factor itself.
/// A winit event filter keeps it right when the window moves to a screen with
/// another display scale, and handles Ctrl +/-/0.
fn text_size_setup(ui: &MainWindow) {
    use slint::winit_030::winit::event::{ElementState, WindowEvent};
    use slint::winit_030::winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};
    use slint::winit_030::{EventResult, WinitWindowAccessor};

    let prefs = Rc::new(RefCell::new(Prefs::load()));
    ui.set_text_size_label(prefs.borrow().text_size_label().into());

    let change = {
        let (ui_w, prefs) = (ui.as_weak(), prefs.clone());
        move |key: TextSizeKey| {
            let Some(ui) = ui_w.upgrade() else { return };
            let mut p = prefs.borrow_mut();
            if key.apply(&mut p) {
                ui.set_text_size_label(p.text_size_label().into());
                if let Some(native) = ui.window().with_winit_window(|w| w.scale_factor() as f32) {
                    set_scale(ui.window(), native * p.zoom());
                }
            }
        }
    };
    ui.on_text_size({
        let change = change.clone();
        move |dir| {
            change(match dir {
                d if d > 0 => TextSizeKey::Larger,
                d if d < 0 => TextSizeKey::Smaller,
                _ => TextSizeKey::Reset,
            })
        }
    });

    let modifiers = Cell::new(ModifiersState::empty());
    let applied = Cell::new(false);
    ui.window().on_winit_window_event(move |window, event| {
        // The first event comes after Slint has set the display scale: add the text size.
        if !applied.replace(true) && prefs.borrow().text_size != myrdc_core::prefs::DEFAULT_TEXT_SIZE {
            if let Some(native) = window.with_winit_window(|w| w.scale_factor() as f32) {
                set_scale(window, native * prefs.borrow().zoom());
            }
        }
        match event {
            // Another display scale (window moved to another screen, or Windows setting changed).
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                set_scale(window, *scale_factor as f32 * prefs.borrow().zoom());
                EventResult::PreventDefault
            }
            WindowEvent::ModifiersChanged(m) => {
                modifiers.set(m.state());
                EventResult::Propagate
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let m = modifiers.get();
                let cmd = if cfg!(target_os = "macos") { m.super_key() } else { m.control_key() };
                if !cmd {
                    return EventResult::Propagate;
                }
                let by_char = match &event.logical_key {
                    Key::Character(c) => TextSizeKey::from_text(c.as_str()),
                    _ => None,
                };
                let key = by_char.or(match event.physical_key {
                    PhysicalKey::Code(KeyCode::NumpadAdd) => Some(TextSizeKey::Larger),
                    PhysicalKey::Code(KeyCode::NumpadSubtract) => Some(TextSizeKey::Smaller),
                    PhysicalKey::Code(KeyCode::Numpad0) => Some(TextSizeKey::Reset),
                    _ => None,
                });
                match key {
                    Some(k) => {
                        change(k);
                        EventResult::PreventDefault
                    }
                    None => EventResult::Propagate,
                }
            }
            _ => EventResult::Propagate,
        }
    });
}

/// Draw the window at `scale` (physical pixels per logical pixel), same window size.
fn set_scale(window: &slint::Window, scale: f32) {
    use slint::platform::WindowEvent;
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
    let size = window.size().to_logical(scale);
    window.dispatch_event(WindowEvent::Resized { size });
    // A larger scale can make the window smaller than its minimum size
    // (MainWindow min-width / min-height). Windows doesn't grow it by itself.
    let (min_w, min_h) = (MIN_WIDTH * scale, MIN_HEIGHT * scale);
    let phys = window.size();
    if (phys.width as f32) < min_w || (phys.height as f32) < min_h {
        window.set_size(slint::PhysicalSize::new(
            phys.width.max(min_w.ceil() as u32),
            phys.height.max(min_h.ceil() as u32),
        ));
    }
}

/// Same as `min-width` / `min-height` of MainWindow in app.slint.
const MIN_WIDTH: f32 = 720.0;
const MIN_HEIGHT: f32 = 440.0;

const ALL_CONNECTIONS: &str = "All connections";

fn update_export_count(ui: &MainWindow, s: &Shared, scope: &str) {
    let group = (scope != ALL_CONNECTIONS).then_some(scope);
    let n = s.borrow().vault.as_ref().map(|v| export::select(&v.data, group).connections.len()).unwrap_or(0);
    ui.set_export_count_text(format!("{n} connection(s)").into());
}

/// Import files and show the result. Returns false when an export password was wrong.
fn run_import(ui: &MainWindow, s: &Shared, files: &[PathBuf], password: Option<&str>) -> bool {
    let mut st = s.borrow_mut();
    let Some(v) = st.vault.as_mut() else { return true };
    let report = import::import_files(&mut v.data, files, password);
    if report.added == 0 && report.errors.iter().any(|e| e.contains("Wrong export password")) {
        return false;
    }
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
        show_draft(ui, &c);
    }
    drop(st);
    refresh_list(ui, s);
    true
}

fn enter_vault(ui: &MainWindow, s: &Shared, v: Vault) {
    ui.set_remembered(v.is_remembered());
    ui.set_auto_lock_minutes(v.data.settings.auto_lock_minutes as i32);
    {
        let mut st = s.borrow_mut();
        st.vault = Some(v);
        st.selected = None;
        st.selected_group = None;
        st.last_activity = Instant::now();
    }
    ui.set_lock_error("".into());
    ui.set_search("".into());
    ui.set_has_draft(false);
    ui.set_selected_group("".into());
    ui.set_status("".into());
    ui.set_locked(false);
    refresh_list(ui, s);
}

fn lock(ui: &MainWindow, s: &Shared) {
    let mut st = s.borrow_mut();
    st.vault = None; // drops and wipes the key
    st.selected = None;
    st.selected_group = None;
    ui.set_creating(!Vault::exists(&st.path));
    ui.set_has_remembered(keystore::load(&st.path).is_some());
    drop(st);
    // Clear everything sensitive from the UI.
    ui.set_draft(ConnData::default());
    ui.set_has_draft(false);
    ui.set_rows(ModelRc::default());
    ui.set_change_pw_open(false);
    ui.set_settings_open(false);
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

/// Select the requested connection, then save a PNG of the window and quit.
#[cfg(feature = "screenshot")]
fn screenshot(ui: &MainWindow, s: &Shared) -> Option<Timer> {
    let req = myrdc_core::screenshot::request()?;
    let (w, h) = myrdc_core::screenshot::SIZE;
    ui.window().set_size(slint::LogicalSize::new(w, h));
    if let Some(name) = &req.select {
        let id = s.borrow().vault.as_ref().and_then(|v| v.data.connections.iter().find(|c| &c.name == name).map(|c| c.id));
        if let Some(id) = id {
            select(ui, s, &id.to_string());
        }
    }
    let ui_w = ui.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::SingleShot, std::time::Duration::from_millis(1500), move || {
        let ui = ui_w.unwrap();
        match ui.window().take_snapshot() {
            Ok(buf) => {
                let res = myrdc_core::screenshot::save_png(&req.out, buf.width(), buf.height(), buf.as_bytes());
                if let Err(e) = res {
                    eprintln!("Could not save screenshot: {e}");
                }
            }
            Err(e) => eprintln!("Could not take screenshot: {e}"),
        }
        let _ = slint::quit_event_loop();
    });
    Some(timer)
}

fn refresh_list(ui: &MainWindow, s: &Shared) {
    let st = s.borrow();
    let Some(v) = st.vault.as_ref() else { return };
    let tree = v.data.tree_rows(&ui.get_search());
    let colors = theme::row_colors(&tree);
    let rows: Vec<ListRow> = tree
        .into_iter()
        .zip(colors)
        .map(|(r, rgb)| match r {
            TreeRow::Group { path, name, depth, expanded, count } => ListRow {
                id: "".into(),
                group_path: path.into(),
                title: name.into(),
                tooltip: "".into(),
                depth: depth as i32,
                expanded,
                count: count as i32,
                color: to_color(rgb),
            },
            TreeRow::Connection { id, name, address, depth } => ListRow {
                id: id.to_string().into(),
                group_path: "".into(),
                title: name.into(),
                tooltip: address.into(),
                depth: depth as i32,
                expanded: false,
                count: 0,
                color: to_color(rgb),
            },
        })
        .collect();
    let mut names: Vec<SharedString> = vec![NO_GROUP.into()];
    names.extend(v.data.all_groups().into_iter().map(SharedString::from));
    ui.set_group_names(ModelRc::new(VecModel::from(names)));
    ui.set_rows(ModelRc::new(VecModel::from(rows)));
}

fn to_color(c: theme::Rgb) -> slint::Color {
    slint::Color::from_rgb_u8(c.0, c.1, c.2)
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
