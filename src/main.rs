use eframe::egui;
use egui::{Button, Color32, RichText, Slider, TextEdit, TextStyle, Vec2};
use rodio::cpal;
use rodio::cpal::traits::HostTrait;
use rodio::DeviceTrait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::AsRef;
use std::fs;
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
    volume: f32,
    outputs: HashMap<String, OutputConfig>,
    sounds: Vec<SoundConfig>,
    shortcuts: ShortcutsConfig,
}

/// Holds audio output configuration
#[derive(Serialize, Deserialize)]
struct OutputConfig {
    volume: f32,
    mute: KeyButton,
}

/// Holds shortcut configuration.
#[derive(Serialize, Deserialize)]
struct ShortcutsConfig {
    pause: KeyButton,
    stop: KeyButton,
    modifier: KeyButton,
}

/// Holds a sound configuration.
#[derive(Serialize, Deserialize, Clone)]
pub struct SoundConfig {
    path: String,
    name: String,
    volume: f32,
    key: KeyButton,
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            name: String::new(),
            volume: 1.0,
            key: KeyButton::default(),
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

#[derive(Clone)]
struct KeyButton {
    pub key: Key,
    listening: bool,
}

impl Serialize for KeyButton {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.key.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for KeyButton {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self {
            key: Key::deserialize(deserializer)?,
            listening: false,
        })
    }
}

impl Default for KeyButton {
    fn default() -> Self {
        Self::new(Key::KEY_RESERVED)
    }
}

impl KeyButton {
    fn new(key: Key) -> Self {
        Self {
            key,
            listening: false,
        }
    }
    fn update(&mut self, ui: &mut egui::Ui, last_key_released: Option<Key>) -> egui::Response {
        let response = if self.listening {
            // Listening for a key release...
            if let Some(key) = last_key_released {
                // We have obtained a last released key. Set the new value and stop listening.
                self.key = key;
                self.listening = false;
                ui.button(if self.key == Key::KEY_RESERVED {
                    "None"
                } else {
                    self.key.as_ref()
                })
            } else {
                // No key has been released.
                ui.button("Binding...")
            }
        } else {
            // We aren't listening.
            ui.button(if self.key == Key::KEY_RESERVED {
                "None"
            } else {
                self.key.as_ref()
            })
        };

        if response.clicked() {
            // When clicked, toggle listening.
            self.listening ^= true;
        }
        if response.secondary_clicked() {
            self.listening = false;
            self.key = Key::KEY_RESERVED;
        }

        response
    }
}

fn toggle_ui(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let desired_size = egui::vec2(50.0, 25.0);
    let (rect, mut response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, *on, ""));

    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool(response.id, *on);
        let visuals = ui.style().interact_selectable(&response, *on);
        let rect = rect.expand(visuals.expansion);
        let radius = 0.5 * rect.height();
        ui.painter().rect(
            rect,
            radius,
            if *on { Color32::GREEN } else { Color32::RED },
            visuals.bg_stroke,
        );
        let circle_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
        let center = egui::pos2(circle_x, rect.center().y);
        ui.painter()
            .circle(center, 0.75 * radius, visuals.bg_fill, visuals.fg_stroke);
    }

    response
}

struct Soundboard {
    config: Config,
    client_manager: RemoteInputClientManager,
    modified: bool,
    config_saver: ConfigSaver,
    output_devices: HashMap<String, OutputDevice>,
    audio_controls: Vec<Arc<AudioControls>>,
    playing: bool,
    enabled: bool,
    settings_window: bool,
    manual_window: bool,
    new_sound: SoundConfig,
    dropped_file: (i64, Option<String>),
}

impl Soundboard {
    const CONFIG_AUTOSAVE: Duration = Duration::from_secs(30);
    const MAX_FRAME_DELAY: Duration = Duration::from_millis(100);

    /// Create a new [`Soundboard`].
    fn new(_: &eframe::CreationContext<'_>) -> Self {
        // Load configuration file.
        let config = load_config().unwrap();

        let mut self_ = Self {
            config,
            client_manager: RemoteInputClientManager::new(),
            modified: false,
            config_saver: ConfigSaver::new(Self::CONFIG_AUTOSAVE),
            output_devices: HashMap::new(),
            audio_controls: Vec::new(),
            playing: true,
            enabled: false,
            settings_window: false,
            manual_window: false,
            new_sound: SoundConfig::default(),
            dropped_file: (0, None),
        };

        for _ in 0..self_.config.sounds.len() {
            self_
                .audio_controls
                .push(Arc::new(AudioControls::new(false, true, 1.0)));
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
                            if let Some(output_config) = self.config.outputs.get(&name) {
                                output_device.set_volume(output_config.volume);
                                output_device.enable();
                            }
                            Some((name, output_device))
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
    fn play_sound(&mut self, filename: &str, controls: &Arc<AudioControls>) {
        for (_, device) in self.output_devices.iter_mut() {
            device.play_sound(filename, controls.clone());
        }
    }
}

impl eframe::App for Soundboard {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let events = self.client_manager.events();
        let suppress_events = self.config.shortcuts.pause.listening
            || self.config.shortcuts.stop.listening
            || self.config.shortcuts.modifier.listening
            || self.config.sounds.iter().any(|s| s.key.listening);
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
                if self.enabled {
                    for (controls, path) in self
                        .config
                        .sounds
                        .iter()
                        .enumerate()
                        .filter_map(|(i, sound)| {
                            if sound.key.key == key {
                                if self.modified {
                                    if self.audio_controls[i].playing() {
                                        self.audio_controls[i].pause()
                                    } else {
                                        self.audio_controls[i].play()
                                    }
                                    self.modified = false;
                                    None
                                } else {
                                    self.audio_controls[i].stop();
                                    self.audio_controls[i] = Arc::new(AudioControls::new(
                                        true,
                                        false,
                                        self.config.volume * sound.volume,
                                    ));
                                    Some((self.audio_controls[i].clone(), sound.path.clone()))
                                }
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<(_, _)>>()
                    {
                        self.play_sound(&path, &controls);
                    }
                }

                for (name, output_config) in &self.config.outputs {
                    if key == output_config.mute.key {
                        self.output_devices[name].toggle_muted();
                    }
                }

                if key == self.config.shortcuts.pause.key {
                    self.playing ^= true;
                    for controls in &self.audio_controls {
                        controls.set_playing(self.playing);
                    }
                }

                if key == self.config.shortcuts.stop.key {
                    self.playing = false;
                    for controls in &self.audio_controls {
                        controls.stop();
                    }
                }

                if key == self.config.shortcuts.modifier.key {
                    self.modified ^= true;
                }
            }
        }

        // Keep track of the dropped file for 5 frames. This is required because the pointer location
        // is unknown while a file is being dragged, so .hovered will always be false when the file is dropped.
        if self.dropped_file.1 == None || self.dropped_file.0 > 5 {
            self.dropped_file.0 = 0;
            self.dropped_file.1 = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .first()
                    .and_then(|f| f.path.clone().and_then(|p| Some(p.display().to_string())))
            });
        } else {
            self.dropped_file.0 += 1;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            // Enable toggle
            if toggle_ui(ui, &mut self.enabled).changed() && self.enabled == false {
                for controls in &self.audio_controls {
                    controls.stop();
                }
            }

            // Connect and disconnect from remote input server.
            if self.client_manager.connected() {
                if ui.button("Disconnect").clicked() {
                    self.client_manager.disconnect();
                }
            } else {
                if ui
                    .add(
                        Button::new(RichText::new("Connect").color(Color32::BLACK))
                            .fill(Color32::RED),
                    )
                    .clicked()
                {
                    self.client_manager.connect(
                        self.config.server_address.clone(),
                        self.config.api_key.clone(),
                    );
                }
            }

            // Settings window
            if ui.button("Settings").clicked() {
                self.settings_window = true;
            }

            // Manual window
            if ui.button("Help / Manual").clicked() {
                self.manual_window = true;
            }

            // Volume slider
            if ui
                .add(Slider::new(&mut self.config.volume, 0.0..=1.0).text("Volume"))
                .changed()
            {
                for (i, control) in self.audio_controls.iter_mut().enumerate() {
                    control.set_volume(self.config.volume * self.config.sounds[i].volume);
                }
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("sounds").num_columns(9).show(ui, |ui| {
                    // Selected output devices
                    for (name, device) in &self.output_devices {
                        if device.muted() {
                            ui.colored_label(Color32::RED, "Muted");
                        } else {
                            ui.colored_label(Color32::GREEN, "Playing");
                        }
                        ui.label(name);

                        // Volume slider
                        if ui
                            .add(
                                Slider::new(
                                    &mut self.config.outputs.get_mut(name).unwrap().volume,
                                    0.0..=1.0,
                                )
                                .text("Volume"),
                            )
                            .changed()
                        {
                            device.set_volume(self.config.outputs[name].volume);
                        }
                        ui.end_row();
                    }

                    // New Sound
                    ui.label("");
                    ui.add(
                        TextEdit::singleline(&mut self.new_sound.name)
                            .min_size([100.0, 10.0].into()),
                    );
                    self.new_sound.key.update(ui, last_key_released);
                    ui.add(Slider::new(&mut self.new_sound.volume, 0.0..=1.0));

                    if ui
                        .add(
                            TextEdit::singleline(&mut self.new_sound.path)
                                .min_size([300.0, 10.0].into()),
                        )
                        .hovered()
                    {
                        if let Some(path) = self.dropped_file.1.take() {
                            self.new_sound.path = path;
                        }
                    }

                    if ui.button("Add").clicked() {
                        self.audio_controls.insert(
                            0,
                            Arc::new(AudioControls::new(
                                false,
                                false,
                                self.new_sound.volume * self.config.volume,
                            )),
                        );
                        self.config.sounds.insert(0, self.new_sound.clone());
                        self.new_sound = SoundConfig::default();
                    }
                    ui.end_row();

                    // Other Sounds
                    let mut i = 0;
                    let mut action = (0, 0, 0); // ((none, remove, move), index a, index b)
                    let length = self.config.sounds.len();

                    for sound in self.config.sounds.iter_mut() {
                        // Playing
                        if self.audio_controls[i].stopped() {
                            ui.colored_label(Color32::RED, "\u{23F9}");
                        } else if self.audio_controls[i].playing() {
                            ui.colored_label(Color32::GREEN, "\u{25B6}");
                        } else {
                            ui.colored_label(Color32::YELLOW, "\u{23F8}");
                        }

                        // Name
                        ui.add(
                            TextEdit::singleline(&mut sound.name).min_size([100.0, 10.0].into()),
                        );

                        // Key
                        sound.key.update(ui, last_key_released);

                        // Volume
                        if ui.add(Slider::new(&mut sound.volume, 0.0..=1.0)).changed() {
                            self.audio_controls[i].set_volume(self.config.volume * sound.volume);
                        }

                        // Path
                        if ui
                            .add(
                                TextEdit::singleline(&mut sound.path)
                                    .min_size([300.0, 10.0].into()),
                            )
                            .hovered()
                        {
                            if let Some(path) = self.dropped_file.1.take() {
                                sound.path = path;
                            }
                        }

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
                        self.audio_controls.remove(action.1);
                    } else if action.0 == 2 {
                        self.config.sounds.swap(action.1, action.2);
                        self.audio_controls.swap(action.1, action.2);
                    }
                });
            });
        });

        let mut settings_window = self.settings_window;
        egui::Window::new("Settings")
            .open(&mut settings_window)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("settings").show(ui, |ui| {
                    // Audio settings
                    ui.heading("Audio");
                    ui.end_row();

                    if ui.button("Reload Devices").clicked() {
                        self.update_output_devices();
                    }
                    ui.end_row();

                    for (name, device) in self.output_devices.iter_mut() {
                        let mut checked = device.enabled();

                        // Enabled checkbox
                        let response = ui.checkbox(&mut checked, name);

                        if let Some(output_config) = self.config.outputs.get_mut(name) {
                            // Mute key bind button
                            output_config.mute.update(ui, last_key_released);
                        }

                        // Add and remove device.
                        if response.changed() {
                            if checked {
                                assert!(
                                    !self.config.outputs.contains_key(name),
                                    "a device in self.config.outputs exists when it should not"
                                );
                                self.config.outputs.insert(
                                    name.clone(),
                                    OutputConfig {
                                        volume: 1.0,
                                        mute: KeyButton::default(),
                                    },
                                );
                                device.enable();
                            } else {
                                self.config.outputs.remove(name);
                                device.disable();
                            }
                        }
                        ui.end_row();
                    }

                    // Remote input server settings
                    ui.heading("Remote Input Server");
                    ui.end_row();
                    ui.label("Server Address");
                    ui.text_edit_singleline(&mut self.config.server_address);
                    ui.end_row();
                    ui.label("API Key");
                    ui.text_edit_singleline(&mut self.config.api_key);
                    ui.end_row();

                    // Shortcuts
                    ui.heading("Shortcuts");
                    ui.end_row();

                    ui.label("Pause");
                    self.config.shortcuts.pause.update(ui, last_key_released);
                    ui.end_row();

                    ui.label("Stop");
                    self.config.shortcuts.stop.update(ui, last_key_released);
                    ui.end_row();

                    ui.label("Modifier");
                    self.config.shortcuts.modifier.update(ui, last_key_released);
                    ui.end_row();
                });
            });
        self.settings_window = settings_window;

        let mut manual_window = self.manual_window;
        egui::Window::new("Manual")
            .open(&mut manual_window)
            .collapsible(false)
            .min_width(700.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(
                        RichText::new(include_str!("manual.txt")).text_style(TextStyle::Monospace),
                    );
                });
            });
        self.manual_window = manual_window;

        let _ = self.config_saver.save(&self.config);

        ctx.request_repaint_after(Self::MAX_FRAME_DELAY);
    }

    fn on_close_event(&mut self) -> bool {
        let _ = self.config_saver.save(&self.config);
        true
    }
}

fn main() {
    let mut native_options = eframe::NativeOptions::default();
    native_options.min_window_size = Some(Vec2::new(850.0, 500.0));
    native_options.drag_and_drop_support = true;
    let _ = eframe::run_native(
        "Soundboard",
        native_options,
        Box::new(|cc| Box::new(Soundboard::new(cc))),
    );
}
