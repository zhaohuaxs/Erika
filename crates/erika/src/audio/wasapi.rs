use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient, IAudioRenderClient,
    IAudioSessionControl, IAudioSessionManager, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::Media::Audio::IMMDeviceEnumerator;

use crate::audio::{
    apply_volume, normalize_volume, AudioClockSnapshot, AudioError, AudioOutputBackend,
    AudioOutputState, AudioPushResult, AudioRingBuffer, AudioRingBufferConfig,
    AudioRingBufferStats, Result,
};
use crate::ffmpeg::{PcmAudioFrame, PcmFormat};

const REFTIMES_PER_SEC: i64 = 10_000_000;
const BUFFER_DURATION_REFTIMES: i64 = REFTIMES_PER_SEC / 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WasapiAudioConfig {
    pub ring_buffer: AudioRingBufferConfig,
}

impl Default for WasapiAudioConfig {
    fn default() -> Self {
        Self {
            ring_buffer: AudioRingBufferConfig {
                capacity_frames: 96_000,
                drop_oldest_on_overflow: true,
            },
        }
    }
}

unsafe impl Send for WasapiInner {}
unsafe impl Sync for WasapiInner {}

struct WasapiInner {
    audio_client: Option<IAudioClient>,
    render_client: Option<IAudioRenderClient>,
    session_control: Option<IAudioSessionControl>,
    buffer: AudioRingBuffer,
    volume: f32,
    state: AudioOutputState,
    format: Option<PcmFormat>,
    frame_size: u32,
}

pub struct WasapiAudioOutput {
    inner: Arc<Mutex<WasapiInner>>,
    render_thread: Option<std::thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl WasapiAudioOutput {
    pub fn new(config: WasapiAudioConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(WasapiInner {
                audio_client: None,
                render_client: None,
                session_control: None,
                buffer: AudioRingBuffer::new(config.ring_buffer),
                volume: 1.0,
                state: AudioOutputState::Stopped,
                format: None,
                frame_size: 0,
            })),
            render_thread: None,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    fn init_audio_client(format: PcmFormat) -> Result<(IAudioClient, IAudioRenderClient, Option<IAudioSessionControl>, u32)> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&windows::Win32::Media::Audio::MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| AudioError::Backend(format!("failed to create device enumerator: {e}")))?;

            let device = enumerator
                .GetDefaultAudioEndpoint(windows::Win32::Media::Audio::eRender, windows::Win32::Media::Audio::eConsole)
                .map_err(|e| AudioError::Backend(format!("failed to get default audio endpoint: {e}")))?;

            let audio_client: IAudioClient = device
                .Activate::<IAudioClient>(CLSCTX_ALL, None)
                .map_err(|e| AudioError::Backend(format!("failed to activate audio client: {e}")))?;

            let wave_format = pcm_to_wave_format(format);

            let flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            audio_client
                .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_DURATION_REFTIMES, 0, &wave_format, None)
                .map_err(|e| AudioError::Backend(format!("failed to initialize audio client: {e}")))?;

            let frame_size = audio_client
                .GetBufferSize()
                .map_err(|e| AudioError::Backend(format!("failed to get buffer size: {e}")))?;

            let render_client: IAudioRenderClient = audio_client
                .GetService()
                .map_err(|e| AudioError::Backend(format!("failed to get render client: {e}")))?;

            let session_control = device
                .Activate::<IAudioSessionManager>(CLSCTX_ALL, None)
                .ok()
                .and_then(|mgr| mgr.GetAudioSessionControl(None, 0).ok());

            Ok((audio_client, render_client, session_control, frame_size))
        }
    }
}

fn pcm_to_wave_format(format: PcmFormat) -> WAVEFORMATEX {
    let block_align = (format.channels as u16) * 4;
    WAVEFORMATEX {
        wFormatTag: 3,
        nChannels: format.channels as u16,
        nSamplesPerSec: format.sample_rate,
        nAvgBytesPerSec: format.sample_rate * block_align as u32,
        nBlockAlign: block_align,
        wBitsPerSample: 32,
        cbSize: 0,
    }
}

impl Drop for WasapiAudioOutput {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = self.render_thread.take() {
            let _ = thread.join();
        }
        unsafe {
            CoUninitialize();
        }
    }
}

impl AudioOutputBackend for WasapiAudioOutput {
    fn configure(&mut self, format: PcmFormat) -> Result<()> {
        let (audio_client, render_client, session_control, frame_size) =
            Self::init_audio_client(format)?;

        let mut inner = self.inner.lock().map_err(|_| {
            AudioError::Backend("audio output lock poisoned".to_string())
        })?;
        inner.audio_client = Some(audio_client);
        inner.render_client = Some(render_client);
        inner.session_control = session_control;
        inner.frame_size = frame_size;
        inner.format = Some(format);
        inner.buffer.configure(format)
    }

    fn start(&mut self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            AudioError::Backend("audio output lock poisoned".to_string())
        })?;

        if let Some(ref audio_client) = inner.audio_client {
            unsafe {
                audio_client
                    .Start()
                    .map_err(|e| AudioError::Backend(format!("failed to start audio client: {e}")))?;
            }
        }

        inner.state = AudioOutputState::Playing;

        if self.render_thread.is_none() {
            self.shutdown.store(false, Ordering::Relaxed);
            let inner_clone = Arc::clone(&self.inner);
            let shutdown_clone = Arc::clone(&self.shutdown);
            let frame_size = inner.frame_size;
            let format = inner.format;

            self.render_thread = Some(std::thread::Builder::new()
                .name("erika-wasapi-render".to_string())
                .spawn(move || {
                    wasapi_render_loop(inner_clone, shutdown_clone, frame_size, format);
                })
                .expect("spawn WASAPI render thread"));
        }

        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            AudioError::Backend("audio output lock poisoned".to_string())
        })?;

        if let Some(ref audio_client) = inner.audio_client {
            unsafe {
                audio_client
                    .Stop()
                    .map_err(|e| AudioError::Backend(format!("failed to stop audio client: {e}")))?;
            }
        }

        inner.state = AudioOutputState::Paused;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = self.render_thread.take() {
            let _ = thread.join();
        }

        let mut inner = self.inner.lock().map_err(|_| {
            AudioError::Backend("audio output lock poisoned".to_string())
        })?;

        if let Some(ref audio_client) = inner.audio_client {
            unsafe {
                let _ = audio_client.Stop();
                let _ = audio_client.Reset();
            }
        }

        inner.state = AudioOutputState::Stopped;
        Ok(())
    }

    fn set_volume(&mut self, volume: f32) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.volume = normalize_volume(volume);
        }
    }

    fn volume(&self) -> f32 {
        self.inner
            .lock()
            .map(|inner| inner.volume)
            .unwrap_or(1.0)
    }

    fn push(&mut self, frame: PcmAudioFrame) -> Result<AudioPushResult> {
        let mut inner = self.inner.lock().map_err(|_| {
            AudioError::Backend("audio output lock poisoned".to_string())
        })?;
        inner.buffer.push_frame(frame)
    }

    fn state(&self) -> AudioOutputState {
        self.inner
            .lock()
            .map(|inner| inner.state)
            .unwrap_or(AudioOutputState::Stopped)
    }

    fn stats(&self) -> AudioRingBufferStats {
        self.inner
            .lock()
            .map(|inner| inner.buffer.stats())
            .unwrap_or_default()
    }

    fn clock_snapshot(&self) -> Option<AudioClockSnapshot> {
        self.inner
            .lock()
            .ok()
            .map(|inner| inner.buffer.clock_snapshot())
    }
}

fn wasapi_render_loop(
    inner: Arc<Mutex<WasapiInner>>,
    shutdown: Arc<AtomicBool>,
    frame_size: u32,
    format: Option<PcmFormat>,
) {
    let channels = format.map(|f| f.channels as usize).unwrap_or(2);
    let render_buffer_frames = frame_size as usize;
    let mut read_buffer = vec![0.0f32; render_buffer_frames * channels];

    while !shutdown.load(Ordering::Relaxed) {
        let (render_client, audio_client, volume) = {
            let Ok(inner) = inner.lock() else {
                break;
            };
            let Some(ref rc) = inner.render_client else {
                break;
            };
            let Some(ref ac) = inner.audio_client else {
                break;
            };
            (rc.clone(), ac.clone(), inner.volume)
        };

        unsafe {
            let padding = match audio_client.GetCurrentPadding() {
                Ok(p) => p,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
            };

            let available = frame_size.saturating_sub(padding) as usize;
            if available == 0 {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }

            let frames_to_render = available.min(render_buffer_frames);
            let samples_needed = frames_to_render * channels;

            if read_buffer.len() < samples_needed {
                read_buffer.resize(samples_needed, 0.0);
            }

            let data_ptr = match render_client.GetBuffer(frames_to_render as u32) {
                Ok(ptr) => ptr,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
            };

            {
                let mut inner = match inner.lock() {
                    Ok(i) => i,
                    Err(_) => {
                        render_client.ReleaseBuffer(frames_to_render as u32, 0).ok();
                        break;
                    }
                };

                for sample in read_buffer.iter_mut() {
                    *sample = 0.0;
                }

                let read_slice = &mut read_buffer[..samples_needed];
                if inner.buffer.read_interleaved(read_slice).is_ok() {
                    apply_volume(read_slice, volume);
                }
            }

            let dst = std::slice::from_raw_parts_mut(data_ptr as *mut f32, samples_needed);
            dst.copy_from_slice(&read_buffer[..samples_needed]);

            if render_client.ReleaseBuffer(frames_to_render as u32, 0).is_err() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        std::thread::sleep(Duration::from_millis(1));
    }
}
