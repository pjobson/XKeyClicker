#![warn(clippy::pedantic)]
#![windows_subsystem = "windows"]

use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    thread::{sleep, spawn},
};

use gtk::{
    gio::ApplicationFlags,
    glib::{self, Type},
    prelude::{ApplicationExt, ApplicationExtManual, BuilderExtManual, TreeViewExt, TreeSelectionExt, GtkListStoreExtManual, TreeViewColumnExt, TreeModelExt as _},
    traits::{ButtonExt, CellRendererToggleExt, EntryExt, GtkWindowExt, WidgetExt, GtkListStoreExt, LabelExt},
    Application, ApplicationWindow, Builder, Button, Entry, ListStore, TreeView, CellRendererText, CellRendererToggle, TreeViewColumn, DrawingArea, Label,
};
use gtk::EditableSignals;
use primitives::{KeyType, NotMut, XKeyClicker, KeyBehavior};
use glib::Sender;
use rdev::{listen, simulate, Event, EventType, Key};

mod primitives;

type ArcXKeyClicker = Arc<XKeyClicker>;

/// Stores the pending Entry reference on the main thread
type PendingEntry = Rc<RefCell<Option<Entry>>>;

/// Message sent from the listener thread to the GTK main thread
#[derive(Debug, Clone)]
enum KeyMessage {
    KeyCaptured { key: Key, key_type: KeyType },
}

fn main() {
    let xkeyclicker = XKeyClicker::new();

    let xkc_handle = xkeyclicker.clone();
    // Spawn auto clicker
    spawn(move || auto_clicker(&xkc_handle));

    let app = Application::new(Some("com.s0ra.xkeyclicker"), ApplicationFlags::default());
    app.connect_activate(move |app| build_ui(app, xkeyclicker.clone()));
    app.run();
}

/// Interruptible sleep that checks state every 100ms
/// Returns false if state became inactive during the sleep
fn interruptible_sleep(xkc_handle: &ArcXKeyClicker, duration: std::time::Duration) -> bool {
    let check_interval = std::time::Duration::from_millis(100);
    let mut remaining = duration;

    while remaining > std::time::Duration::ZERO {
        let sleep_time = remaining.min(check_interval);
        sleep(sleep_time);

        // Check if we should stop
        if !*xkc_handle.state.lock().unwrap() {
            return false;
        }

        remaining = remaining.saturating_sub(sleep_time);
    }
    true
}

fn on_start(xkc_handle: &ArcXKeyClicker) -> bool {
    // Apply start delay with interruptible sleep
    let start_delay = *xkc_handle.start_delay.lock().unwrap();
    if start_delay > 0 {
        if !interruptible_sleep(xkc_handle, std::time::Duration::from_secs(start_delay)) {
            return false;
        }
    }

    let hold_keys = xkc_handle.get_hold_keys();
    let mut held_keys = xkc_handle.held_keys.lock().unwrap();

    for key in &hold_keys {
        if simulate(&EventType::KeyPress(*key)).is_ok() {
            held_keys.push(*key);
        }
    }

    *xkc_handle.click_index.lock().unwrap() = 0;
    *xkc_handle.current_count.lock().unwrap() = 0;
    true
}

fn on_stop(xkc_handle: &ArcXKeyClicker) {
    let mut held_keys = xkc_handle.held_keys.lock().unwrap();

    // Release in reverse order
    while let Some(key) = held_keys.pop() {
        let _ = simulate(&EventType::KeyRelease(key));
    }
}

/// Returns true if we should continue clicking, false if repeat count reached
fn click_next_key(xkc_handle: &ArcXKeyClicker) -> bool {
    let click_keys = xkc_handle.get_click_keys();
    if click_keys.is_empty() {
        return true;
    }

    let mut index = xkc_handle.click_index.lock().unwrap();
    let key = click_keys[*index % click_keys.len()];

    let _ = simulate(&EventType::KeyPress(key));
    let _ = simulate(&EventType::KeyRelease(key));

    *index = (*index + 1) % click_keys.len();

    // Increment and check repeat count
    let mut current = xkc_handle.current_count.lock().unwrap();
    *current += 1;

    let repeat_count = *xkc_handle.repeat_count.lock().unwrap();
    if repeat_count > 0 && *current >= repeat_count {
        return false;
    }

    true
}

fn auto_clicker(xkc_handle: &ArcXKeyClicker) {
    loop {
        let current_state = *xkc_handle.state.lock().unwrap();
        let prev_state = *xkc_handle.prev_state.lock().unwrap();

        // Detect state transitions
        if current_state && !prev_state {
            // off -> on transition
            let started_successfully = on_start(xkc_handle);
            if !started_successfully {
                // User stopped during start delay, don't mark as started
                continue;
            }
            *xkc_handle.prev_state.lock().unwrap() = true;
        } else if !current_state && prev_state {
            // on -> off transition
            on_stop(xkc_handle);
            *xkc_handle.prev_state.lock().unwrap() = false;
        }

        // Re-check state after potential on_start
        let current_state = *xkc_handle.state.lock().unwrap();

        if current_state {
            let delay = xkc_handle.cooldown.lock().unwrap().as_duration();
            let should_continue = click_next_key(xkc_handle);

            if !should_continue {
                // Repeat count reached, stop automatically
                *xkc_handle.state.lock().unwrap() = false;
            } else {
                // Use interruptible sleep for cooldown
                interruptible_sleep(xkc_handle, delay);
            }
        } else {
            // Small sleep to avoid busy-waiting when inactive
            sleep(std::time::Duration::from_millis(10));
        }
    }
}

fn refresh_list_store(list_store: &ListStore, xkc_handle: &ArcXKeyClicker) {
    list_store.clear();
    let actions = xkc_handle.key_actions.lock().unwrap();
    for (i, action) in actions.iter().enumerate() {
        let iter = list_store.append();
        list_store.set(&iter, &[
            (0, &(i as u32)),
            (1, &format!("{:?}", action.key)),
            (2, &(action.behavior == KeyBehavior::Hold)),
        ]);
    }
}

fn build_ui(app: &Application, xkc_handle: ArcXKeyClicker) {
    let builder = Builder::from_string(include_str!("xkeyclicker.ui"));
    let window: ApplicationWindow = builder.object("window").unwrap();

    window.set_application(Some(app));

    // Create glib channel for thread-safe communication from listener to GTK main thread
    let (key_sender, key_receiver) = glib::MainContext::channel::<KeyMessage>(glib::PRIORITY_DEFAULT);

    // Spawn keybind listener
    let xkc_handle_for_listener = xkc_handle.clone();
    spawn(move || {
        listen(move |e| {
            keybind(&e, &key_sender, &xkc_handle_for_listener);
        })
        .unwrap();
    });

    macro_rules! time_entry {
        ($time_type:tt, $default_cooldown:tt) => {
            let $time_type: Entry = builder
                .object(&format!("time_{}", stringify!($time_type)))
                .unwrap();
            let xkc_handle_clone = xkc_handle.clone();

            $time_type.connect_changed(move |entry| {
                if let Ok(cooldown) = entry.buffer().text().parse::<u64>() {
                    xkc_handle_clone.cooldown.lock().unwrap().$time_type = cooldown;
                } else if !entry.buffer().text().is_empty() {
                    entry.set_text("0");
                    xkc_handle_clone.cooldown.lock().unwrap().$time_type = 0;
                }
            });
        };
    }

    time_entry!(mins, 0);
    time_entry!(secs, 0);
    time_entry!(millis, 100);
    time_entry!(micros, 0);

    // Manual start button
    let manual_start_button: Button = builder.object("manual_start_button").unwrap();

    // Start delay entry
    let start_delay_entry: Entry = builder.object("start_delay_entry").unwrap();
    let xkc_handle_clone = xkc_handle.clone();
    let manual_start_button_clone = manual_start_button.clone();
    start_delay_entry.connect_changed(move |entry| {
        let text = entry.buffer().text();
        if text.is_empty() {
            // Treat empty field as zero
            *xkc_handle_clone.start_delay.lock().unwrap() = 0;
            manual_start_button_clone.set_sensitive(false);
        } else if let Ok(delay) = text.parse::<u64>() {
            *xkc_handle_clone.start_delay.lock().unwrap() = delay;
            // Enable button only if delay > 0
            manual_start_button_clone.set_sensitive(delay > 0);
        } else {
            entry.set_text("0");
            *xkc_handle_clone.start_delay.lock().unwrap() = 0;
            manual_start_button_clone.set_sensitive(false);
        }
    });

    // Manual start button click handler
    let xkc_handle_for_start = xkc_handle.clone();
    manual_start_button.connect_clicked(move |_| {
        let mut state = xkc_handle_for_start.state.lock().unwrap();
        if !*state {
            *state = true;
        }
    });

    // Repeat count entry
    let repeat_count_entry: Entry = builder.object("repeat_count_entry").unwrap();
    let xkc_handle_clone = xkc_handle.clone();
    repeat_count_entry.connect_changed(move |entry| {
        let text = entry.buffer().text();
        if text.is_empty() {
            // Treat empty field as zero
            *xkc_handle_clone.repeat_count.lock().unwrap() = 0;
        } else if let Ok(count) = text.parse::<u64>() {
            *xkc_handle_clone.repeat_count.lock().unwrap() = count;
        } else {
            entry.set_text("0");
            *xkc_handle_clone.repeat_count.lock().unwrap() = 0;
        }
    });

    let start_keybind_button: Button = builder.object("start_keybind").unwrap();
    let keybind_entry: Entry = builder.object("keybind_entry").unwrap();

    // Create shared pending entry storage (GTK main thread only)
    let pending_entry: PendingEntry = Rc::new(RefCell::new(None));

    let xkc_handle_clone = xkc_handle.clone();
    let pending_entry_clone = pending_entry.clone();

    start_keybind_button.connect_clicked(move |_| {
        set_keybind(
            &keybind_entry,
            &pending_entry_clone,
            &xkc_handle_clone,
            KeyType::Keybind,
        );
    });

    // Set up the key list TreeView
    let key_list_store = ListStore::new(&[Type::U32, Type::STRING, Type::BOOL]);
    let key_tree_view: TreeView = builder.object("key_tree_view").unwrap();
    key_tree_view.set_model(Some(&key_list_store));

    // Column 0: Index (hidden, used for internal tracking)
    // Column 1: Key name
    let key_name_renderer = CellRendererText::new();
    let key_name_column = TreeViewColumn::new();
    key_name_column.set_title("Key");
    key_name_column.set_expand(true);
    key_name_column.pack_start(&key_name_renderer, true);
    key_name_column.add_attribute(&key_name_renderer, "text", 1);
    key_tree_view.append_column(&key_name_column);

    // Column 2: Hold toggle
    let hold_renderer = CellRendererToggle::new();
    hold_renderer.set_activatable(true);

    let list_store_clone = key_list_store.clone();
    let xkc_handle_clone = xkc_handle.clone();
    hold_renderer.connect_toggled(move |_, path| {
        if let Some(iter) = list_store_clone.iter(&path) {
            let index: u32 = list_store_clone.value(&iter, 0).get().unwrap_or(0);
            xkc_handle_clone.toggle_behavior(index as usize);
            refresh_list_store(&list_store_clone, &xkc_handle_clone);
        }
    });

    let hold_column = TreeViewColumn::new();
    hold_column.set_title("Hold");
    hold_column.pack_start(&hold_renderer, false);
    hold_column.add_attribute(&hold_renderer, "active", 2);
    key_tree_view.append_column(&hold_column);

    // Add Key button
    let add_key_button: Button = builder.object("add_key_button").unwrap();
    let key_status_entry: Entry = builder.object("key_status_entry").unwrap();

    let xkc_handle_for_add = xkc_handle.clone();
    let pending_entry_for_add = pending_entry.clone();
    add_key_button.connect_clicked(move |_| {
        set_keybind(
            &key_status_entry,
            &pending_entry_for_add,
            &xkc_handle_for_add,
            KeyType::AddKey,
        );
    });

    // Remove Key button
    let remove_key_button: Button = builder.object("remove_key_button").unwrap();
    let tree_view_clone = key_tree_view.clone();
    let list_store_clone = key_list_store.clone();
    let xkc_handle_for_remove = xkc_handle.clone();
    remove_key_button.connect_clicked(move |_| {
        let selection = tree_view_clone.selection();
        if let Some((model, iter)) = selection.selected() {
            let index: u32 = model.value(&iter, 0).get().unwrap_or(0);
            xkc_handle_for_remove.remove_key_action(index as usize);
            refresh_list_store(&list_store_clone, &xkc_handle_for_remove);
        }
    });

    // Move Up button
    let move_up_button: Button = builder.object("move_up_button").unwrap();
    let tree_view_clone = key_tree_view.clone();
    let list_store_clone = key_list_store.clone();
    let xkc_handle_for_up = xkc_handle.clone();
    move_up_button.connect_clicked(move |_| {
        let selection = tree_view_clone.selection();
        if let Some((model, iter)) = selection.selected() {
            let index: u32 = model.value(&iter, 0).get().unwrap_or(0);
            xkc_handle_for_up.move_key_up(index as usize);
            refresh_list_store(&list_store_clone, &xkc_handle_for_up);
        }
    });

    // Move Down button
    let move_down_button: Button = builder.object("move_down_button").unwrap();
    let tree_view_clone = key_tree_view.clone();
    let list_store_clone = key_list_store.clone();
    let xkc_handle_for_down = xkc_handle.clone();
    move_down_button.connect_clicked(move |_| {
        let selection = tree_view_clone.selection();
        if let Some((model, iter)) = selection.selected() {
            let index: u32 = model.value(&iter, 0).get().unwrap_or(0);
            xkc_handle_for_down.move_key_down(index as usize);
            refresh_list_store(&list_store_clone, &xkc_handle_for_down);
        }
    });

    // Handle key capture messages from the listener thread (runs on GTK main thread)
    let xkc_handle_for_receiver = xkc_handle.clone();
    let pending_entry_for_receiver = pending_entry.clone();
    key_receiver.attach(None, move |msg| {
        match msg {
            KeyMessage::KeyCaptured { key, key_type } => {
                // Check if the key type matches what we're waiting for
                let mut should_recv = xkc_handle_for_receiver.should_recv.lock().unwrap();

                if *should_recv == key_type {
                    // Take ownership of pending entry
                    let entry_opt = pending_entry_for_receiver.borrow_mut().take();

                    match key_type {
                        KeyType::AddKey => {
                            xkc_handle_for_receiver.add_key_action(key);
                            if let Some(entry) = entry_opt {
                                entry.set_text(&format!("Added: {:?}", key));
                            }
                        }
                        KeyType::Keybind => {
                            *xkc_handle_for_receiver.keybind.lock().unwrap() = key;
                            if let Some(entry) = entry_opt {
                                entry.set_text(&format!("{:?}", key));
                            }
                        }
                        KeyType::None => {}
                    }
                    *should_recv = KeyType::None;
                }
            }
        }
        glib::Continue(true)
    });

    // Status indicator setup
    let status_indicator: DrawingArea = builder.object("status_indicator").unwrap();
    let status_label: Label = builder.object("status_label").unwrap();

    // Set up drawing for the status indicator
    let xkc_for_draw = xkc_handle.clone();
    status_indicator.connect_draw(move |_, cr| {
        let is_active = *xkc_for_draw.state.lock().unwrap();

        if is_active {
            cr.set_source_rgb(0.0, 0.8, 0.0); // Green
        } else {
            cr.set_source_rgb(0.5, 0.5, 0.5); // Gray
        }

        // Draw a filled circle
        cr.arc(8.0, 8.0, 7.0, 0.0, 2.0 * std::f64::consts::PI);
        let _ = cr.fill();

        gtk::Inhibit(false)
    });

    // Poll for changes to refresh the list and status indicator
    let list_store_poll = key_list_store.clone();
    let xkc_poll = xkc_handle.clone();
    let status_indicator_poll = status_indicator.clone();
    let status_label_poll = status_label.clone();
    let mut prev_poll_state = false;
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        let actions_len = xkc_poll.key_actions.lock().unwrap().len();
        let store_len = list_store_poll.iter_n_children(None) as usize;
        if actions_len != store_len {
            refresh_list_store(&list_store_poll, &xkc_poll);
        }

        // Update status indicator
        let current_state = *xkc_poll.state.lock().unwrap();
        if current_state != prev_poll_state {
            status_indicator_poll.queue_draw();
            status_label_poll.set_text(if current_state { "Active" } else { "Inactive" });
            prev_poll_state = current_state;
        }

        gtk::glib::Continue(true)
    });

    window.show_all();
}

fn set_keybind(
    key_entry: &Entry,
    pending_entry: &PendingEntry,
    xkc_handle: &ArcXKeyClicker,
    key_type: KeyType,
) {
    // Store the key type in the shared state
    *xkc_handle.should_recv.lock().unwrap() = key_type;
    // Store the entry reference on the main thread (GTK-safe)
    *pending_entry.borrow_mut() = Some(key_entry.clone());
    key_entry.set_text("Press a key...");
}

fn keybind(event: &Event, key_sender: &Sender<KeyMessage>, xkc_handle: &ArcXKeyClicker) {
    if let Event {
        time: _,
        name: _,
        event_type: EventType::KeyPress(key),
    } = event
    {
        let should_recv = *xkc_handle.should_recv.lock().unwrap();
        match should_recv {
            KeyType::AddKey | KeyType::Keybind => {
                // Send key event to the GTK main thread for safe UI updates
                let _ = key_sender.send(KeyMessage::KeyCaptured {
                    key: *key,
                    key_type: should_recv,
                });
            }
            KeyType::None => {
                // Check if this is the toggle keybind
                if *key == *xkc_handle.keybind.lock().unwrap() {
                    xkc_handle.state.lock().unwrap().not_mut();
                }
            }
        }
    }
}
