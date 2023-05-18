use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{prelude::*, BufReader};
use std::time::SystemTime;
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
#[derive(Serialize, Deserialize)]
struct SoundConfig {
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

/// Format a [`SystemTime`] as T+{ms} or T-{ms} relative to the current system time.
fn format_timestamp(timestamp: SystemTime) -> String {
    match timestamp.elapsed() {
        Ok(duration) => format!("T-{}ms", duration.as_millis()),
        Err(system_time_error) => format!("T+{}ms", system_time_error.duration().as_millis()),
    }
}

fn main() {
    // Load configuration file.
    let config = load_config().unwrap();
    println!(
        "[Remote Input Client] Connecting to remote input server {}.",
        &config.server_address
    );

    // Connect to the remote input server.
    let mut stream = std::net::TcpStream::connect(&config.server_address).unwrap();
    println!(
        "[Remote Input Client] Connected to remote input server {}.",
        config.server_address
    );

    // Send the API key to the remote input server.
    let api_key = [config.api_key.as_bytes(), &[0x00u8]].concat();
    println!(
        "[Remote Input Client] Sent {} byte API key.",
        stream.write(&api_key).expect("unable to send API key")
    );

    // Receive events from the remote input server.
    // Events are [`InputEventWrapper`] serialized by [`postcard`] and encoded by COBS.
    let mut buffer_reader = BufReader::new(&mut stream);
    let mut event_buffer = Vec::new();
    loop {
        // Receive data.
        event_buffer.clear();
        buffer_reader
            .read_until(0x00, &mut event_buffer)
            .expect("unable to read event");

        // The event should end with 0x00 because it is encoded by COBS.
        if !event_buffer.ends_with(&[0x00]) {
            println!("[Remote Input Client] Connection lost.");
            break;
        }

        // Deserialize event.
        let event_data = event_buffer.as_mut_slice();
        println!(
            "[Remote Input Client] Received event: {}.",
            as_hex::as_hex(event_data)
        );
        match postcard::from_bytes_cobs::<InputEventWrapper>(event_data) {
            Err(deserialize_error) => {
                println!("[Remote Input Client] Failed to deserialize event: {deserialize_error}.")
            }
            Ok(event_wrapper) => match event_wrapper.as_event() {
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
            },
        };
    }
}
