use eframe::egui;
use rodio::buffer;
use rodio::{source::Source, Decoder, OutputStream, OutputStreamHandle, Sink};
use serde::{Deserialize, Serialize};
use std::convert::AsRef;
use std::io::{prelude::*, BufReader};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime};
use std::{fs, thread};
mod as_hex;
mod event;
use event::*;

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

/// Holds information about an input event. Serialized using postcard and sent to clients.
/// Enum values can be found in <https://github.com/torvalds/linux/blob/master/include/uapi/linux/input-event-codes.h>
/// Fields:
/// - `timestamp`: a `std::time::SystemTime` associated with the event
/// - `event_type`: the raw type (e.g., a key press)
/// - `code`: the raw code (e.g., corresponding to a certain key)
/// - `value`: the raw value (e.g., 1 for a key press and 0 for a key release)
#[derive(Deserialize)]
struct InputEventWrapper {
    timestamp: std::time::SystemTime,
    event_type: u16,
    code: u16,
    value: i32,
}

impl InputEventWrapper {
    /// Returns the [`EventType`] of this [`InputEventWrapper`] if it exists.
    fn as_event_type(&self) -> Option<EventType> {
        EventType::from_repr(self.event_type)
    }

    /// Returns the [`Event`] of this [`InputEventWrapper`] if it exists.
    fn as_event(&self) -> Option<Event> {
        Some(match EventType::from_repr(self.event_type)? {
            EventType::EV_SYN => Event::Synchronization(Synchronization::from_repr(self.code)?),
            EventType::EV_KEY => Event::Key(Key::from_repr(self.code)?),
            EventType::EV_REL => Event::RelativeAxis(RelativeAxis::from_repr(self.code)?),
            EventType::EV_ABS => Event::AbsoluteAxis(AbsoluteAxis::from_repr(self.code)?),
            EventType::EV_MSC => Event::Miscellaneous(Miscellaneous::from_repr(self.code)?),
            EventType::EV_SW => Event::Switch(Switch::from_repr(self.code)?),
            EventType::EV_LED => Event::LED(LED::from_repr(self.code)?),
            EventType::EV_SND => Event::Sound(Sound::from_repr(self.code)?),
            EventType::EV_REP => Event::AutoRepeat(AutoRepeat::from_repr(self.code)?),
            EventType::EV_FF => Event::ForceFeedback(ForceFeedback::from_repr(self.code)?),
            EventType::EV_PWR => return None,
            EventType::EV_FF_STATUS => {
                Event::ForceFeedbackStatus(ForceFeedbackStatus::from_repr(self.code)?)
            }
        })
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

struct RemoteInputClientManager {
    remote_input_thread: Option<thread::JoinHandle<()>>,
    event_receiver: Option<Receiver<InputEventWrapper>>,
}

impl RemoteInputClientManager {
    /// Create a new remote input client manager. Nothing will be done until `connect` is called.
    fn new() -> Self {
        Self {
            remote_input_thread: None,
            event_receiver: None,
        }
    }

    /// Connect to the remote input server in a new thread.
    fn connect(&mut self, server_address: String, api_key: String) {
        let (event_sender, event_receiver) = mpsc::channel();
        self.event_receiver = Some(event_receiver);
        self.remote_input_thread = Some(thread::spawn(move || {
            let mut remote_input_client =
                match RemoteInputClient::connect(server_address.clone(), api_key) {
                    Some(r) => r,
                    None => {
                        println!("[Remote Input Client {server_address}] Unable to connect.");
                        return;
                    }
                };
            while let Some(event) = remote_input_client.process_event() {
                if event_sender.send(event).is_err() {
                    println!("[Remote Input Client {server_address}] Local channel disconnected.");
                    return;
                }
            }
            println!("[Remote Input Client {server_address}] Server disconnected.");
        }));
    }

    /// Disconnect the [`RemoteInputClient`].
    fn disconnect(&mut self) {
        self.event_receiver = None;
        self.remote_input_thread = None;
    }

    /// Check if the [`RemoteInputClient`] is connected.
    fn connected(&self) -> bool {
        self.event_receiver.is_some()
            && self
                .remote_input_thread
                .as_ref()
                .is_some_and(|h| !h.is_finished())
    }

    /// Retrieve a list of new input events since this was last called.
    /// This will be emptied when disconnected.
    fn events(&self) -> Vec<InputEventWrapper> {
        match self.event_receiver.as_ref() {
            Some(r) => r.try_iter().collect(),
            None => Vec::new(),
        }
    }
}

struct RemoteInputClient {
    buffer_reader: BufReader<TcpStream>,
    event_buffer: Vec<u8>,
    server_address: String,
}

impl RemoteInputClient {
    fn connect(server_address: String, api_key: String) -> Option<RemoteInputClient> {
        println!(
            "[Remote Input Client {server_address}] Connecting to remote input server {}.",
            server_address
        );

        // Connect to the remote input server.
        let mut stream = match std::net::TcpStream::connect(server_address.clone()) {
            Err(error) => {
                println!("[Remote Input Client {server_address}] Error connecting to remote input server {server_address}: {error}");
                return None;
            }
            Ok(stream) => stream,
        };
        println!("[Remote Input Client {server_address}] Connected to remote input server {server_address}.");

        // Send the API key to the remote input server.
        let api_key = [api_key.as_bytes(), &[0x00u8]].concat();
        match stream.write(&api_key) {
            Ok(n) if n == 0 => {
                println!(
                    "[Remote Input Client {server_address}] Sent 0 bytes of API key. Connection is likely closed."
                );
                return None;
            }
            Ok(n) => println!(
                "[Remote Input Client {server_address}] Sent {n} bytes of {} byte API key.",
                api_key.len()
            ),
            Err(error) => {
                println!("[Remote Input Client {server_address}] Unable to send API key: {error}");
                return None;
            }
        }

        // Receive events from the remote input server.
        // Events are [`InputEventWrapper`] serialized by [`postcard`] and encoded by COBS.
        let buffer_reader = BufReader::new(stream);
        let event_buffer = Vec::new();

        Some(RemoteInputClient {
            buffer_reader,
            event_buffer,
            server_address,
        })
    }

    fn process_event(&mut self) -> Option<InputEventWrapper> {
        let address = &self.server_address;

        // Receive data.
        self.event_buffer.clear();
        match self.buffer_reader.read_until(0x00, &mut self.event_buffer) {
            Ok(n) if n == 0 => {
                println!(
                    "[Remote Input Client {address}] Read 0 bytes of data. Connection is likely closed."
                );
                return None;
            }
            Ok(_) => {}
            Err(error) => {
                println!("[Remote Input Client {address}] Unable to read event: {error}.");
            }
        }

        // Deserialize event.
        let event_data = self.event_buffer.as_mut_slice();
        println!(
            "[Remote Input Client {address}] Received event: {}.",
            as_hex::as_hex(event_data)
        );
        match postcard::from_bytes_cobs::<InputEventWrapper>(event_data) {
            Err(deserialize_error) => {
                println!("[Remote Input Client {address}] Failed to deserialize event: {deserialize_error}.");
                None
            }
            Ok(event_wrapper) => {
                match event_wrapper.as_event() {
                    Some(enumerated_event) => {
                        println!(
                            "[Remote Input Client {address}] Deserialized enumerated event: timestamp: {}, event_type: {}, code: {}, value: {}.",
                            format_timestamp(event_wrapper.timestamp), event_wrapper.as_event_type().unwrap().as_ref(), enumerated_event.code_as_ref(), event_wrapper.value
                        );
                    }
                    None => {
                        println!(
                            "[Remote Input Client {address}] Deserialized undefined event: timestamp: {}, event_type: {}, code: {}, value: {}.",
                            format_timestamp(event_wrapper.timestamp), event_wrapper.event_type, event_wrapper.code, event_wrapper.value
                        );
                    }
                };
                Some(event_wrapper)
            }
        }
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
