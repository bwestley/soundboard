use eframe::egui;
use rodio::buffer;
use rodio::{source::Source, Decoder, OutputStream, OutputStreamHandle, Sink};
use serde::{Deserialize, Serialize};
use std::convert::AsRef;
use std::time::{Duration, SystemTime};
use std::fs;
mod as_hex;
mod event;
use event::*;
mod input;
use input::*;

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
    keycode: u16,
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
}

impl Soundboard {
    fn new(_: &eframe::CreationContext<'_>) -> Self {
        // Load configuration file.
        let config = load_config().unwrap();

        // Spawn [`remote_input_client`].
        let server_address = config.server_address.clone();
        let api_key = config.api_key.clone();
        let mut client_manager = RemoteInputClientManager::new();
        client_manager.connect(server_address, api_key);

        Self {
            config,
            client_manager,
            pause_shortcut: KeyButton::new(),
            play_shortcut: KeyButton::new(),
            disable_shortcut: KeyButton::new(),
            enable_shortcut: KeyButton::new(),
            config_saver: ConfigSaver::new(Duration::from_secs(30)),
        }
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
            for _event in events {
                // TODO: Audio

                /*let (_stream, stream_handle) = OutputStream::try_default().unwrap();
                let file = BufReader::new(File::open("../fart with extra reverb.mp3").unwrap());
                let source = Decoder::new(file).unwrap();
                stream_handle.play_raw(source.convert_samples()).unwrap();*/
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Settings");
            ui.label("Volume");
            ui.add(egui::Slider::new(&mut self.config.volume, 0.0..=1.0).text("Volume"));
            ui.label("Server Address");
            ui.text_edit_singleline(&mut self.config.server_address);
            ui.label("API Key");
            ui.text_edit_singleline(&mut self.config.api_key);
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
            ui.heading("Shortcuts");
            ui.label("Pause");
            self.pause_shortcut
                .update(ui, &mut self.config.shortcuts.pause, last_key_released);
            ui.label("Play");
            self.play_shortcut
                .update(ui, &mut self.config.shortcuts.play, last_key_released);
            ui.label("Disable");
            self.disable_shortcut
                .update(ui, &mut self.config.shortcuts.disable, last_key_released);
            ui.label("Enable");
            self.enable_shortcut
                .update(ui, &mut self.config.shortcuts.enable, last_key_released);
        });

        let _ = self.config_saver.save(&self.config);
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
      - Select Sound Output
      - Select Notification Output
      - Sounds
        - Add Sound
        - Remove Sound
        - Edit Sound
        - Re-order Sound
    - Sound
        - Play
        - Pause
        - Disable
        - Enable
    */
}
