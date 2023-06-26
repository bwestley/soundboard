use eframe::egui;
use egui::Slider;
use rodio::cpal;
use rodio::cpal::traits::HostTrait;
use rodio::DeviceTrait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::AsRef;
use std::fs;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
mod as_hex;
mod event;
use event::*;
mod input;
use input::*;
mod audio;
use audio::*;

/// Holds configuration values read from config.toml.
#[derive(Serialize, Deserialize)]
struct Config {
    server_address: String,
    api_key: String,
    volume: f64,
    sound_outputs: Vec<String>,
    notification_output: String,
    sounds: Vec<SoundConfig>,
    shortcuts: ShortcutsConfig,
}

/// Holds shortcut configuration.
#[derive(Serialize, Deserialize)]
struct ShortcutsConfig {
    pause: Key,
    play: Key,
    disable: Key,
    enable: Key,
}

/// Holds a sound configuration.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SoundConfig {
    path: String,
    name: String,
    volume: f64,
    key: Key,
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            name: String::new(),
            volume: 1.0,
            key: Key::KEY_RESERVED,
        }
    }
}

/// Get the path of the configuration file path.
/// [this executable's directory]/config.toml
fn get_config_file_path() -> Result<std::path::PathBuf, String> {
    match std::env::current_exe() {
        Err(exe_path_error) => {
            return Err(format!(
                "Unable to obtain executable directory: {exe_path_error}."
            ))
        }
        Ok(exe_path) => match exe_path.parent() {
            None => return Err("Unable to obtain executable directory.".to_string()),
            Some(parent_dir) => Ok(parent_dir.join("config.toml")),
        },
    }
}

/// Load the toml configuration from [`get_config_file_path`].
fn load_config() -> Result<Config, String> {
    let config_file_path = get_config_file_path()?;
    println!(
        "[Configuration Loader] Loading configuration file \"{}\".",
        config_file_path.display()
    );

    match fs::read_to_string(&config_file_path) {
        Ok(config_data) => match toml::from_str(&config_data) {
            Err(error) => {
                println!(
                    "[Configuration Loader] Unable to deserialize configuration file: {error}."
                );
                Err(format!(
                    "Unable to deserialize configuration file: {error}."
                ))
            }
            Ok(config) => Ok(config),
        },
        Err(read_error) => {
            println!("Unable to open configuration file: {read_error}. Installing default.");
            if let Err(write_error) =
                fs::write(&config_file_path, include_str!("default_config.toml"))
            {
                println!("[Configuration Loader] Unable to install default configuration file: {write_error}.");
                return Err(format!(
                    "Unable to install default configuration file: {write_error}."
                ));
            }
            match fs::read_to_string(&config_file_path) {
                Err(read_error) => {
                    println!("[Configuration Loader] Unable to open newly created configuration file: {read_error}.");
                    return Err(format!(
                        "Unable to open newly created configuration file: {read_error}."
                    ));
                }
                Ok(serialized_config) => match toml::from_str(&serialized_config) {
                    Err(deserialize_error) => {
                        println!("[Configuration Loader] Unable to deserialize default configuration file: {deserialize_error}.");
                        Err(format!("Unable to deserialize default configuration file: {deserialize_error}."))
                    }
                    Ok(config) => Ok(config),
                },
            }
        }
    }
}

struct ConfigSaver {
    last_serialized: String,
    last_saved: SystemTime,
    autosave_interval: Duration,
}

impl ConfigSaver {
    fn new(autosave_interval: Duration) -> Self {
        Self {
            last_serialized: String::new(),
            last_saved: SystemTime::now(),
            autosave_interval,
        }
    }

    /// Save the toml configuration to [`get_config_file_path`].
    /// Returns true if saved, false if not saved, or a string describing an error.
    fn save(&mut self, config: &Config) -> Result<bool, String> {
        if SystemTime::now() - self.autosave_interval < self.last_saved {
            return Ok(false);
        }

        match toml::to_string_pretty(config) {
            Err(error) => {
                println!("[Configuration Saver] Unable to serialize configuration file: {error}.");
                Err(format!("Unable to serialize configuration file: {error}."))
            }
            Ok(serialized_config) => {
                if serialized_config == self.last_serialized {
                    return Ok(false);
                }
                self.last_serialized = serialized_config;
                self.last_saved = SystemTime::now();
                let config_file_path = get_config_file_path()?;
                println!(
                    "[Configuration Saver] Saving configuration file \"{}\".",
                    config_file_path.display()
                );
                match fs::write(&config_file_path, &self.last_serialized) {
                    Err(error) => {
                        println!(
                            "[Configuration Saver] Unable to write configuration file: {error}."
                        );
                        Err(format!("Unable to write configuration file: {error}."))
                    }
                    Ok(_) => Ok(true),
                }
            }
        }
    }
}

/// Format a [`SystemTime`] as T+{ms} or T-{ms} relative to the current system time.
fn format_timestamp(timestamp: SystemTime) -> String {
    match timestamp.elapsed() {
        Ok(duration) => format!("T-{}ms", duration.as_millis()),
        Err(system_time_error) => format!("T+{}ms", system_time_error.duration().as_millis()),
    }
}

struct KeyButton {
    listening: bool,
}

impl KeyButton {
    fn new() -> Self {
        Self { listening: false }
    }
    fn update(
        &mut self,
        ui: &mut egui::Ui,
        value: &mut Key,
        last_key_released: Option<Key>,
    ) -> egui::Response {
        let response = if self.listening {
            // Listening for a key release...
            if let Some(key) = last_key_released {
                // We have obtained a last released key. Set the new value and stop listening.
                *value = key;
                self.listening = false;
                ui.button(if *value == Key::KEY_RESERVED {
                    "None"
                } else {
                    value.as_ref()
                })
            } else {
                // No key has been released.
                ui.button("Binding...")
            }
        } else {
            // We aren't listening.
            ui.button(if *value == Key::KEY_RESERVED {
                "None"
            } else {
                value.as_ref()
            })
        };

        if response.clicked() {
            // When clicked, toggle listening.
            self.listening ^= true;
        }
        if response.secondary_clicked() {
            self.listening = false;
            *value = Key::KEY_RESERVED;
        }

        response
    }
}

struct Soundboard {
    config: Config,
    client_manager: RemoteInputClientManager,
    pause_shortcut: KeyButton,
    play_shortcut: KeyButton,
    disable_shortcut: KeyButton,
    enable_shortcut: KeyButton,
    config_saver: ConfigSaver,
    output_devices: Vec<OutputDevice>,
    sound_controls: HashMap<String, Vec<Arc<AtomicBool>>>,
    new_sound: SoundConfig,
    sound_key_buttons: Vec<KeyButton>,
}

impl Soundboard {
    /// Create a new [`Soundboard`].
    fn new(_: &eframe::CreationContext<'_>) -> Self {
        // Load configuration file.
        let config = load_config().unwrap();

        // Spawn [`remote_input_client`].
        let server_address = config.server_address.clone();
        let api_key = config.api_key.clone();
        let mut client_manager = RemoteInputClientManager::new();
        client_manager.connect(server_address, api_key);

        let mut self_ = Self {
            config,
            client_manager,
            pause_shortcut: KeyButton::new(),
            play_shortcut: KeyButton::new(),
            disable_shortcut: KeyButton::new(),
            enable_shortcut: KeyButton::new(),
            config_saver: ConfigSaver::new(Duration::from_secs(30)),
            output_devices: Vec::new(),
            sound_controls: HashMap::new(),
            sound_key_buttons: vec![KeyButton::new()],
            new_sound: SoundConfig::default(),
        };

        for _ in 0..self_.config.sounds.len() {
            self_.sound_key_buttons.push(KeyButton::new())
        }
        self_.update_output_devices();

        self_
    }

    /// Update the list of audio output devices.
    fn update_output_devices(&mut self) {
        let host = cpal::default_host();
        self.output_devices.clear();
        match host.output_devices() {
            Ok(devices) => {
                println!("[Soundboard] Found output devices.");
                self.output_devices
                    .extend(devices.filter_map(|device| match device.name() {
                        Ok(name) => {
                            let mut output_device = OutputDevice::new(device);
                            if self.config.sound_outputs.contains(&name) {
                                output_device.enable();
                            }
                            Some(output_device)
                        }
                        Err(error) => {
                            println!("[Soundboard] Error finding device name: {error}.");
                            None
                        }
                    }));
            }
            Err(error) => {
                println!("[Soundboard] Error finding output devices: {error}.");
            }
        }
    }

    /// Play the audio file at `filename` on all output devices.
    fn play_sound(&mut self, filename: &str) {
        self.sound_controls.insert(
            filename.to_owned(),
            self.output_devices
                .iter_mut()
                .filter_map(|device| device.play_sound(filename))
                .collect(),
        );
    }
}

impl eframe::App for Soundboard {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let events = self.client_manager.events();
        let suppress_events = self.pause_shortcut.listening
            || self.play_shortcut.listening
            || self.disable_shortcut.listening
            || self.enable_shortcut.listening;
        let last_key_released = events
            .iter()
            .filter_map(|input_event| {
                if input_event.event_type == EventType::EV_KEY as u16 && input_event.value == 0 {
                    Key::from_repr(input_event.code)
                } else {
                    None
                }
            })
            .last();

        if !suppress_events {
            for key in events.iter().filter_map(|event| {
                if event.event_type == EventType::EV_KEY as u16
                    && event.value == 0
                    && event.code != Key::KEY_RESERVED as u16
                {
                    // The event is a key release that is not `KEY_RESERVED`.
                    Key::from_repr(event.code)
                } else {
                    None
                }
            }) {
                for path in self
                    .config
                    .sounds
                    .iter()
                    .filter_map(|sound| {
                        if sound.key == key {
                            Some(sound.path.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                {
                    self.play_sound(&path);
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Grid::new("sounds").show(ui, |ui| {
                ui.heading("Sounds");
                ui.end_row();

                // New Sound
                ui.text_edit_singleline(&mut self.new_sound.name);
                self.sound_key_buttons[0].update(ui, &mut self.new_sound.key, last_key_released);
                ui.add(Slider::new(&mut self.new_sound.volume, 0.0..=1.0));
                ui.text_edit_singleline(&mut self.new_sound.path);
                if ui.button("Add").clicked() {
                    self.config.sounds.insert(0, self.new_sound.clone());
                    self.new_sound = SoundConfig::default();
                    self.sound_key_buttons.insert(0, KeyButton::new());
                }
                ui.end_row();

                // Other Sounds
                let mut i = 0;
                let mut action = (0, 0, 0); // ((none, remove, move), index a, index b)
                // INVESTIGATE 035166276322b3f2324bd8b97ffcedc63fa8419f
                let length = self.config.sounds.len();

                for sound in self.config.sounds.iter_mut() {
                    // Name
                    ui.text_edit_singleline(&mut sound.name);

                    // Key
                    self.sound_key_buttons[i + 1].update(ui, &mut sound.key, last_key_released);

                    // Volume
                    ui.add(Slider::new(&mut sound.volume, 0.0..=1.0));

                    // Path
                    ui.text_edit_singleline(&mut sound.path);

                    // Remove Sound
                    if ui.button("Remove").clicked() {
                        action = (1, i, 0);
                    }

                    // Move Sound
                    if i > 0 && ui.button("^").clicked() {
                        action = (2, i, i - 1);
                    }
                    if i < length - 1 && ui.button("v").clicked() {
                        action = (2, i, i + 1)
                    }

                    ui.end_row();

                    i += 1;
                }

                // Remove or re-order a sound.
                if action.0 == 1 {
                    drop(self.config.sounds.remove(action.1));
                    self.sound_key_buttons.remove(action.1 + 1); // because [0] is new_sound
                } else if action.0 == 2 {
                    self.config.sounds.swap(action.1, action.2);
                    self.sound_key_buttons.swap(action.1 + 1, action.2 + 1); // because [0] is new_sound
                }
            });
            egui::Grid::new("settings").show(ui, |ui| {
                // Audio settings
                ui.heading("Audio");
                ui.end_row();
                if ui.button("Reload Devices").clicked() {
                    self.update_output_devices();
                }

                for device in self.output_devices.iter_mut() {
                    let mut checked = device.enabled();
                    let name = device.name();
                    if ui.checkbox(&mut checked, name).changed() {
                        println!("{name} {checked} {:?}", self.config.sound_outputs);
                        if checked {
                            assert!(!self.config.sound_outputs.contains(name), "a device in self.config.sound_outputs exists when it should not");
                            self.config.sound_outputs.push(name.clone());
                            device.enable()
                        } else {
                            self.config.sound_outputs.remove(
                                self.config.sound_outputs.iter()
                                .position(|x| x == name)
                                .expect("a device in self.config.sound_outputs does not exist when it should")
                            );
                            device.disable()
                        }
                    }
                    ui.end_row();
                };

                ui.label("Volume");
                ui.add(Slider::new(&mut self.config.volume, 0.0..=1.0));
                ui.end_row();

                // Remote input server settings
                ui.heading("Remote Input Server");
                ui.end_row();
                ui.label("Server Address");
                ui.text_edit_singleline(&mut self.config.server_address);
                ui.end_row();
                ui.label("API Key");
                ui.text_edit_singleline(&mut self.config.api_key);
                ui.end_row();

                if self.client_manager.connected() {
                    if ui.button("Disconnect").clicked() {
                        self.client_manager.disconnect();
                    }
                } else {
                    if ui.button("Connect").clicked() {
                        self.client_manager.connect(
                            self.config.server_address.clone(),
                            self.config.api_key.clone(),
                        );
                    }
                }
                ui.end_row();

                // Shortcuts
                ui.heading("Shortcuts");
                ui.end_row();

                ui.label("Pause");
                self.pause_shortcut
                    .update(ui, &mut self.config.shortcuts.pause, last_key_released);
                ui.end_row();

                ui.label("Play");
                self.play_shortcut
                    .update(ui, &mut self.config.shortcuts.play, last_key_released);
                ui.end_row();

                ui.label("Disable");
                self.disable_shortcut
                    .update(ui, &mut self.config.shortcuts.disable, last_key_released);
                ui.end_row();

                ui.label("Enable");
                self.enable_shortcut
                    .update(ui, &mut self.config.shortcuts.enable, last_key_released);
                ui.end_row();
            });
        });

        let _ = self.config_saver.save(&self.config);
    }

    fn on_close_event(&mut self) -> bool {
        let _ = self.config_saver.save(&self.config);
        true
    }
}

fn main() {
    let native_options = eframe::NativeOptions::default();
    let _ = eframe::run_native(
        "Soundboard",
        native_options,
        Box::new(|cc| Box::new(Soundboard::new(cc))),
    );

    /*
    TODO
    - GUI
      - Select Notification Output
    - Sound
        - Play
        - Pause
        - Disable
        - Enable
    */
}
