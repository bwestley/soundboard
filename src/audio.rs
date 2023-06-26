use rodio::{Decoder, DeviceTrait, OutputStream, OutputStreamHandle, Source};
use std::{
    fs::File,
    io::BufReader,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

pub struct OutputDevice {
    device: rodio::Device,
    name: String,
    enabled: bool,
    stream: Option<OutputStream>,
    stream_handle: Option<OutputStreamHandle>,
}

impl OutputDevice {
    pub fn new(device: rodio::Device) -> Self {
        Self {
            name: device.name().unwrap_or_else(|_| "[Unknown]".to_string()),
            device,
            enabled: false,
            stream: None,
            stream_handle: None,
        }
    }

    /// Create [`OutputStream`] and [`OutputStreamHandle`].
    pub fn enable(&mut self) {
        // Do nothing if already enabled.
        if self.enabled {
            return;
        }

        match OutputStream::try_from_device(&self.device) {
            Err(error) => {
                println!(
                    "[Audio] Unable to build an output stream from device {}: {error}.",
                    self.name
                );
                self.enabled = false;
            }
            Ok((stream, stream_handle)) => {
                self.stream = Some(stream);
                self.stream_handle = Some(stream_handle);
                self.enabled = true;
            }
        }
    }

    /// Drop [`OutputStream`] and [`OutputStreamHandle`].
    pub fn disable(&mut self) {
        // Do nothing if not enabled.
        if !self.enabled {
            return;
        }

        // Drop stream_handle and stream.
        drop(self.stream_handle.take());
        drop(self.stream.take());
        self.enabled = false;
    }

    /// Return self.enabled.
    #[inline]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Return &self.name.
    #[inline]
    pub fn name(&self) -> &String {
        &self.name
    }

    /// Play the audio file at `filename`.
    pub fn play_sound(&mut self, filename: &str) -> Option<Arc<AtomicBool>> {
        // Do nothing if not enabled.
        if !self.enabled {
            return None;
        }

        // Load audio file.
        let file = BufReader::new(match File::open(filename) {
            Err(error) => {
                println!("[Audio] Unable to read file {filename}: {error}.");
                return None;
            }
            Ok(file) => file,
        });

        // Decode file and setup audio pipeline.
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let source = match Decoder::new(file) {
            Err(error) => {
                println!("[Audio] Unable to decode file {filename}: {error}.");
                return None;
            }
            Ok(source) => source,
        }
        .convert_samples()
        .stoppable()
        .periodic_access(Duration::from_millis(200), move |src| {
            // Stop the sound if the [`AtomicBool`] returned from `play_sound` is set to false.
            if stop.load(Ordering::SeqCst) {
                src.stop()
            }
        });

        // Play audio.
        match self
            .stream_handle
            .as_ref()
            .expect("self.stream_handle is None when self.enabled is true")
            .play_raw(source)
        {
            Ok(()) => Some(stop_clone),
            Err(error) => {
                println!("[Audio] Unable to play {filename}: {error}.");
                None
            }
        }
    }
}
