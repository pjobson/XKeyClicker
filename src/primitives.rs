use std::{
    ops::Not,
    sync::{Arc, Mutex},
    time::Duration,
};

use rdev::Key;

pub trait NotMut {
    fn not_mut(&mut self);
}

// For inverting a bool without assigning old value to a variable
impl NotMut for bool {
    fn not_mut(&mut self) {
        *self = self.not();
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum KeyBehavior {
    #[default]
    Click,
    Hold,
}

#[derive(Debug, Clone, Copy)]
pub struct KeyAction {
    pub key: Key,
    pub behavior: KeyBehavior,
}

impl KeyAction {
    pub fn new(key: Key) -> Self {
        Self {
            key,
            behavior: KeyBehavior::Click,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum KeyType {
    AddKey,
    Keybind,
    #[default]
    None,
}

pub struct XKeyClicker {
    pub keybind: Mutex<Key>,
    pub should_recv: Mutex<KeyType>,
    pub state: Mutex<bool>,
    pub cooldown: Mutex<Cooldown>,
    pub start_delay: Mutex<u64>,
    pub repeat_count: Mutex<u64>,
    pub current_count: Mutex<u64>,
    pub key_actions: Mutex<Vec<KeyAction>>,
    pub click_index: Mutex<usize>,
    pub held_keys: Mutex<Vec<Key>>,
    pub prev_state: Mutex<bool>,
}

impl Default for XKeyClicker {
    fn default() -> Self {
        Self {
            keybind: Mutex::new(Key::F7),
            should_recv: Mutex::default(),
            state: Mutex::default(),
            cooldown: Mutex::default(),
            start_delay: Mutex::new(0),
            repeat_count: Mutex::new(0),
            current_count: Mutex::new(0),
            key_actions: Mutex::new(Vec::new()),
            click_index: Mutex::new(0),
            held_keys: Mutex::new(Vec::new()),
            prev_state: Mutex::new(false),
        }
    }
}

impl XKeyClicker {
    pub fn new() -> Arc<XKeyClicker> {
        Arc::default()
    }

    pub fn add_key_action(&self, key: Key) {
        self.key_actions.lock().unwrap().push(KeyAction::new(key));
    }

    pub fn remove_key_action(&self, index: usize) {
        let mut actions = self.key_actions.lock().unwrap();
        if index < actions.len() {
            actions.remove(index);
        }
    }

    pub fn move_key_up(&self, index: usize) {
        let mut actions = self.key_actions.lock().unwrap();
        if index > 0 && index < actions.len() {
            actions.swap(index, index - 1);
        }
    }

    pub fn move_key_down(&self, index: usize) {
        let mut actions = self.key_actions.lock().unwrap();
        if index + 1 < actions.len() {
            actions.swap(index, index + 1);
        }
    }

    pub fn toggle_behavior(&self, index: usize) {
        let mut actions = self.key_actions.lock().unwrap();
        if index < actions.len() {
            actions[index].behavior = match actions[index].behavior {
                KeyBehavior::Click => KeyBehavior::Hold,
                KeyBehavior::Hold => KeyBehavior::Click,
            };
        }
    }

    pub fn get_click_keys(&self) -> Vec<Key> {
        self.key_actions
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a.behavior == KeyBehavior::Click)
            .map(|a| a.key)
            .collect()
    }

    pub fn get_hold_keys(&self) -> Vec<Key> {
        self.key_actions
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a.behavior == KeyBehavior::Hold)
            .map(|a| a.key)
            .collect()
    }
}

#[derive(Debug)]
pub struct Cooldown {
    pub mins: u64,
    pub secs: u64,
    pub millis: u64,
    pub micros: u64,
}

impl Default for Cooldown {
    fn default() -> Self {
        Self {
            mins: 0,
            secs: 0,
            millis: 100,
            micros: 0,
        }
    }
}

impl Cooldown {
    pub fn as_duration(&self) -> Duration {
        Duration::from_secs(self.mins * 60) // There's no Duration::from_mins() ¯\_(ツ)_/¯
            + Duration::from_secs(self.secs)
            + Duration::from_millis(self.millis)
            + Duration::from_micros(self.micros)
    }
}

