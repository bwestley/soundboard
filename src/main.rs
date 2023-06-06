use iced::widget::{button, slider, text, text_input, Column, Row};
use iced::{executor, Alignment, Application, Command, Element, Settings, Theme};
use rodio::buffer;
use rodio::{source::Source, Decoder, OutputStream, OutputStreamHandle, Sink};
use serde::{Deserialize, Serialize};
use std::io::{prelude::*, BufReader};
use std::net::TcpStream;
use std::sync::mpsc;
use std::time::SystemTime;
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
    pause: u16,
    play: u16,
    disable: u16,
    enable: u16,
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
/// Enum values can be found in https://github.com/torvalds/linux/blob/master/include/uapi/linux/input-event-codes.h
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
            Err(error) => Err(format!(
                "Unable to deserialize configuration file: {error}."
            )),
            Ok(config) => Ok(config),
        },
        Err(read_error) => {
            println!("Unable to open configuration file: {read_error}. Installing default.");
            if let Err(write_error) =
                fs::write(&config_file_path, include_str!("default_config.toml"))
            {
                return Err(format!(
                    "Unable to install default configuration file: {write_error}."
                ));
            }
            match fs::read_to_string(&config_file_path) {
                Err(read_error) => {
                    return Err(format!(
                        "Unable to open newly created configuration file: {read_error}."
                    ))
                }
                Ok(serialized_config) => match toml::from_str(&serialized_config) {
                    Err(deserialize_error) => Err(format!(
                        "Unable to deserialize default configuration file: {deserialize_error}."
                    )),
                    Ok(config) => Ok(config),
                },
            }
        }
    }
}

/// Save the toml configuration to [`get_config_file_path`].
fn save_config(config: &Config) -> Result<(), String> {
    let config_file_path = get_config_file_path()?;
    println!(
        "[Configuration Saver] Saving configuration file \"{}\".",
        config_file_path.display()
    );

    match toml::to_string_pretty(config) {
        Err(error) => return Err(format!("Unable to serialize configuration file: {error}.")),
        Ok(serialized_config) => match fs::write(&config_file_path, serialized_config) {
            Err(error) => Err(format!("Unable to write configuration file: {error}.")),
            Ok(_) => Ok(()),
        },
    }
}

/// Format a [`SystemTime`] as T+{ms} or T-{ms} relative to the current system time.
fn format_timestamp(timestamp: SystemTime) -> String {
    match timestamp.elapsed() {
        Ok(duration) => format!("T-{}ms", duration.as_millis()),
        Err(system_time_error) => format!("T+{}ms", system_time_error.duration().as_millis()),
    }
}

struct RemoteInputClient {
    buffer_reader: BufReader<TcpStream>,
    event_buffer: Vec<u8>,
}

impl RemoteInputClient {
    fn new(server_address: String, api_key: String) -> RemoteInputClient {
        println!(
            "[Remote Input Client] Connecting to remote input server {}.",
            server_address
        );

        // Connect to the remote input server.
        let mut stream = std::net::TcpStream::connect(server_address.clone()).unwrap();
        println!(
            "[Remote Input Client] Connected to remote input server {}.",
            server_address
        );

        // Send the API key to the remote input server.
        let api_key = [api_key.as_bytes(), &[0x00u8]].concat();
        println!(
            "[Remote Input Client] Sent {} byte API key.",
            stream.write(&api_key).expect("unable to send API key")
        );

        // Receive events from the remote input server.
        // Events are [`InputEventWrapper`] serialized by [`postcard`] and encoded by COBS.
        let buffer_reader = BufReader::new(stream);
        let event_buffer = Vec::new();

        RemoteInputClient {
            buffer_reader,
            event_buffer,
        }
    }

    fn process_event(&mut self) -> Option<InputEventWrapper> {
        // Receive data.
        self.event_buffer.clear();
        self.buffer_reader
            .read_until(0x00, &mut self.event_buffer)
            .expect("unable to read event");

        // The event should end with 0x00 because it is encoded by COBS.
        if !self.event_buffer.ends_with(&[0x00]) {
            println!("[Remote Input Client] Connection lost.");
            return None;
        }

        // Deserialize event.
        let event_data = self.event_buffer.as_mut_slice();
        println!(
            "[Remote Input Client] Received event: {}.",
            as_hex::as_hex(event_data)
        );
        match postcard::from_bytes_cobs::<InputEventWrapper>(event_data) {
            Err(deserialize_error) => {
                println!("[Remote Input Client] Failed to deserialize event: {deserialize_error}.");
                None
            }
            Ok(event_wrapper) => {
                match event_wrapper.as_event() {
                    Some(enumerated_event) => {
                        println!(
                            "[Remote Input Client] Deserialized enumerated event: timestamp: {}, event_type: {}, code: {}, value: {}.",
                            format_timestamp(event_wrapper.timestamp), event_wrapper.as_event_type().unwrap().as_ref(), enumerated_event.code_as_ref(), event_wrapper.value
                        );
                    }
                    None => {
                        println!(
                            "[Remote Input Client] Deserialized undefined event: timestamp: {}, event_type: {}, code: {}, value: {}.",
                            format_timestamp(event_wrapper.timestamp), event_wrapper.event_type, event_wrapper.code, event_wrapper.value
                        );
                    }
                };
                Some(event_wrapper)
            }
        }
    }
}

struct Soundboard {
    config: Config,
}

#[derive(Debug, Clone)]
pub enum Message {
    ConfigChanged(ConfigChanged),
    ShortcutChanged(ShortcutChanged),
    SoundChanged(SoundChanged),
}

#[derive(Debug, Clone)]
pub enum ConfigChanged {
    ServerAddress(String),
    ApiKey(String),
    Volume(f64),
    AddSoundOutput(String),
    RemoveSoundOutput(usize),
    NotificationOutput(String),
}

#[derive(Debug, Clone)]
pub enum ShortcutChanged {
    Pause(u16),
    Play(u16),
    Disable(u16),
    Enable(u16),
}

#[derive(Debug, Clone)]
pub enum SoundChanged {
    AddSound(SoundConfig),
    RemoveSound(usize),
    EditSound(usize, SoundConfig),
}

impl Application for Soundboard {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;
    type Theme = Theme;

    fn new(_flags: ()) -> (Self, Command<Self::Message>) {
        (
            Self {
                config: load_config().unwrap(),
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        String::from("A cool application")
    }

    fn update(&mut self, message: Self::Message) -> Command<Self::Message> {
        match message {
            Message::ConfigChanged(config_changed) => match config_changed {
                ConfigChanged::ServerAddress(server_address) => {
                    self.config.server_address = server_address
                }
                ConfigChanged::ApiKey(api_key) => self.config.api_key = api_key,
                ConfigChanged::Volume(volume) => self.config.volume = volume,
                ConfigChanged::AddSoundOutput(name) => self.config.sound_outputs.push(name),
                ConfigChanged::RemoveSoundOutput(index) => {
                    if index <= self.config.sound_outputs.len() {
                        self.config.sound_outputs.remove(index);
                    }
                }
                ConfigChanged::NotificationOutput(name) => self.config.notification_output = name,
            },
            Message::ShortcutChanged(shortcut_changed) => match shortcut_changed {
                ShortcutChanged::Pause(key_code) => self.config.shortcuts.pause = key_code,
                ShortcutChanged::Play(key_code) => self.config.shortcuts.play = key_code,
                ShortcutChanged::Disable(key_code) => self.config.shortcuts.disable = key_code,
                ShortcutChanged::Enable(key_code) => self.config.shortcuts.enable = key_code,
            },
            Message::SoundChanged(sound_changed) => match sound_changed {
                SoundChanged::AddSound(sound_config) => self.config.sounds.push(sound_config),
                SoundChanged::RemoveSound(index) => {
                    if index <= self.config.sound_outputs.len() {
                        self.config.sounds.remove(index);
                    }
                }
                SoundChanged::EditSound(index, sound_config) => {
                    if index <= self.config.sound_outputs.len() {
                        if index <= self.config.sound_outputs.len() {
                            self.config.sounds.remove(index);
                            self.config.sounds.insert(index, sound_config);
                        }
                    }
                }
            },
        };
        Command::none()
    }

    fn view(&self) -> Element<Self::Message> {
        /*Column::<Self::Message>::with_children(vec![
            button("+").on_press(Self::Message::Increment),
            text(self.value).size(50),
            button("-").on_press(Self::Message::Decrement),
        ])
        .into()*/
        Row::new()
            .push(
                Column::new()
                    .push(text("Server Address").size(30))
                    .push(text("API Key").size(30))
                    .push(text("Volume").size(30))
                    .spacing(10),
            )
            .push(
                Column::new()
                    .push(
                        text_input("", &self.config.server_address).on_input(|input| {
                            Self::Message::ConfigChanged(ConfigChanged::ServerAddress(input))
                        }),
                    )
                    .push(text_input("", &self.config.api_key).on_input(|input| {
                        Self::Message::ConfigChanged(ConfigChanged::ApiKey(input))
                    }))
                    .push(slider(
                        0..=100,
                        (self.config.volume * 100.0).clamp(0.0, 100.0) as i32,
                        |volume| {
                            Self::Message::ConfigChanged(ConfigChanged::Volume(
                                volume as f64 / 100f64,
                            ))
                        },
                    ))
                    .spacing(10),
            )
            .spacing(10)
            .padding(20)
            .align_items(Alignment::Center)
            .into()
    }

    fn theme(&self) -> Self::Theme {
        Self::Theme::Dark
    }
}

fn main() {
    // Load configuration file.
    let config = load_config().unwrap();
    // Spawn [`remote_input_client`].
    let (input_event_tx, input_event_rx) = mpsc::channel();
    let server_address = config.server_address.clone();
    let api_key = config.api_key.clone();
    let _ = thread::spawn(move || {
        let mut remote_input_client = RemoteInputClient::new(server_address, api_key);
        loop {
            input_event_tx
                .send(remote_input_client.process_event())
                .expect("unable to send event");
        }
    });

    // TODO: GUI
    // TODO: Periodic and on-exit config saving
    // TODO: Audio

    if let Err(error) = Soundboard::run(Settings::default()) {
        println!("[Main] Application error: {error}.");
    }

    /*let (_stream, stream_handle) = OutputStream::try_default().unwrap();
    let file = BufReader::new(File::open("../fart with extra reverb.mp3").unwrap());
    let source = Decoder::new(file).unwrap();
    stream_handle.play_raw(source.convert_samples()).unwrap();*/
}
