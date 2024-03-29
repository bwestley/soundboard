use rodio::{Decoder, DeviceTrait, OutputStream, OutputStreamHandle, Source};
use std::{
    fs::File,
    io::BufReader,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub struct AudioControls {
    playing: AtomicBool,
    stopped: AtomicBool,
    volume: Mutex<f32>,
}

impl Default for AudioControls {
    fn default() -> Self {
        Self {
            playing: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            volume: Mutex::new(0.0),
        }
    }
}

impl AudioControls {
    pub fn new(playing: bool, stopped: bool, volume: f32) -> Self {
        Self {
            playing: AtomicBool::new(playing),
            stopped: AtomicBool::new(stopped),
            volume: Mutex::new(volume),
        }
    }

    pub fn play(&self) {
        self.playing.store(true, Ordering::SeqCst);
    }

    pub fn pause(&self) {
        self.playing.store(false, Ordering::SeqCst);
    }

    pub fn stop(&self) {
        self.playing.store(false, Ordering::SeqCst);
        self.stopped.store(true, Ordering::SeqCst);
    }

    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub fn set_playing(&self, playing: bool) {
        self.playing.store(playing, Ordering::SeqCst);
    }

    pub fn playing(&self) -> bool {
        self.playing.load(Ordering::SeqCst)
    }

    pub fn set_volume(&self, volume: f32) {
        *self.volume.lock().unwrap() = volume;
    }

    pub fn get_volume(&self) -> f32 {
        *self.volume.lock().unwrap()
    }
}

pub struct OutputDevice {
    device: rodio::Device,
    name: String,
    enabled: bool,
    volume: Arc<Mutex<f32>>,
    muted: Arc<AtomicBool>,
    stream: Option<OutputStream>,
    stream_handle: Option<OutputStreamHandle>,
}

impl OutputDevice {
    pub fn new(device: rodio::Device) -> Self {
        Self {
            name: device.name().unwrap_or_else(|_| "[Unknown]".to_string()),
            device,
            enabled: false,
            volume: Arc::new(Mutex::new(0.0)),
            muted: Arc::new(AtomicBool::new(false)),
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

    /// Play the audio file at `filename` and return true on success.
    pub fn play_sound(&mut self, filename: &str, controls: Arc<AudioControls>) -> bool {
        // Do nothing if not enabled.
        if !self.enabled {
            return false;
        }

        // Load audio file.
        let file = BufReader::new(match File::open(filename) {
            Err(error) => {
                println!("[Audio] Unable to read file {filename}: {error}.");
                return false;
            }
            Ok(file) => file,
        });

        // Decode file and setup audio pipeline.
        let device_volume = self.volume.clone();
        let muted = self.muted.clone();
        let source = match Decoder::new(file) {
            Err(error) => {
                println!("[Audio] Unable to decode file {filename}: {error}.");
                return false;
            }
            Ok(source) => source,
        }
        .convert_samples()
        .stoppable()
        .pausable(false)
        .amplify(1.0)
        .periodic_access(Duration::from_millis(200), move |src| {
            // Update with [`AudioControls`].
            if controls.stopped.load(Ordering::SeqCst) {
                src.inner_mut().inner_mut().stop();
            }

            src.inner_mut()
                .set_paused(!controls.playing.load(Ordering::SeqCst));
            if muted.load(Ordering::SeqCst) {
                src.set_factor(0.0);
            } else {
                //let a = *device_volume.lock().unwrap();
                //let b = *device_volume.lock().unwrap();
                src.set_factor(10_f32.powf(
                    (*controls.volume.lock().unwrap() + *device_volume.lock().unwrap()) / 20.0,
                ));
                //println!("{}", 10_f32.powf((a + b) / 20.0));
            }
        });

        // Play audio.
        match self
            .stream_handle
            .as_ref()
            .expect("self.stream_handle is None when self.enabled is true")
            .play_raw(source)
        {
            Ok(()) => true,
            Err(error) => {
                println!("[Audio] Unable to play {filename}: {error}.");
                false
            }
        }
    }

    /// Set volume.
    pub fn set_volume(&self, volume: f32) {
        *self.volume.lock().unwrap() = volume
    }

    /// Get volume.
    #[inline]
    pub fn volume(&self) -> f32 {
        *self.volume.lock().unwrap()
    }

    /// Toggle muted.
    pub fn toggle_muted(&self) {
        self.muted.fetch_xor(true, Ordering::AcqRel);
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::SeqCst);
    }

    /// Get muted.
    #[inline]
    pub fn muted(&self) -> bool {
        self.muted.load(Ordering::SeqCst)
    }
}
