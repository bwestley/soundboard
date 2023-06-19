use crate::as_hex::as_hex;
use crate::event::*;
use crate::format_timestamp;
use serde::Deserialize;
use std::io::{prelude::*, BufReader};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// Holds information about an input event. Serialized using postcard and sent to clients.
/// Enum values can be found in <https://github.com/torvalds/linux/blob/master/include/uapi/linux/input-event-codes.h>
/// Fields:
/// - `timestamp`: a `std::time::SystemTime` associated with the event
/// - `event_type`: the raw type (e.g., a key press)
/// - `code`: the raw code (e.g., corresponding to a certain key)
/// - `value`: the raw value (e.g., 1 for a key press and 0 for a key release)
#[derive(Deserialize)]
pub struct InputEventWrapper {
    pub timestamp: std::time::SystemTime,
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}

impl InputEventWrapper {
    /// Returns the [`EventType`] of this [`InputEventWrapper`] if it exists.
    pub fn as_event_type(&self) -> Option<EventType> {
        EventType::from_repr(self.event_type)
    }

    /// Returns the [`Event`] of this [`InputEventWrapper`] if it exists.
    pub fn as_event(&self) -> Option<Event> {
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

pub struct RemoteInputClientManager {
    remote_input_thread: Option<thread::JoinHandle<()>>,
    event_receiver: Option<Receiver<InputEventWrapper>>,
}

impl RemoteInputClientManager {
    /// Create a new remote input client manager. Nothing will be done until `connect` is called.
    pub fn new() -> Self {
        Self {
            remote_input_thread: None,
            event_receiver: None,
        }
    }

    /// Connect to the remote input server in a new thread.
    pub fn connect(&mut self, server_address: String, api_key: String) {
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
    pub fn disconnect(&mut self) {
        self.event_receiver = None;
        self.remote_input_thread = None;
    }

    /// Check if the [`RemoteInputClient`] is connected.
    pub fn connected(&self) -> bool {
        self.event_receiver.is_some()
            && self
                .remote_input_thread
                .as_ref()
                .is_some_and(|h| !h.is_finished())
    }

    /// Retrieve a list of new input events since this was last called.
    /// This will be emptied when disconnected.
    pub fn events(&self) -> Vec<InputEventWrapper> {
        match self.event_receiver.as_ref() {
            Some(r) => r.try_iter().collect(),
            None => Vec::new(),
        }
    }
}

pub struct RemoteInputClient {
    buffer_reader: BufReader<TcpStream>,
    event_buffer: Vec<u8>,
    server_address: String,
}

impl RemoteInputClient {
    pub fn connect(server_address: String, api_key: String) -> Option<RemoteInputClient> {
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

    pub fn process_event(&mut self) -> Option<InputEventWrapper> {
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
            as_hex(event_data)
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
