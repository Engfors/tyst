//! macOS system audio through a Core Audio process tap (SPEC 10.1, macOS 14.4+): a private,
//! global, mono tap of every process's output, inside a private aggregate device whose clock is
//! the default output device. Needs `NSAudioCaptureUsageDescription`; macOS asks the user once.
//!
//! When the default output changes (headphones plugged in), [`SystemAudioTap::poll`] rebuilds the
//! aggregate device on the new output, so the session keeps running.

use std::ffi::{CStr, c_void};
use std::ptr::NonNull;
use std::sync::mpsc::Sender;
use std::time::Instant;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop,
    AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap, AudioHardwareDestroyAggregateDevice,
    AudioHardwareDestroyProcessTap, AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
    CATapDescription, kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey, kAudioAggregateDeviceSubDeviceListKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey,
    kAudioDevicePropertyDeviceUID, kAudioHardwarePropertyDefaultSystemOutputDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, kAudioSubDeviceUIDKey, kAudioSubTapDriftCompensationKey,
    kAudioSubTapUIDKey, kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};

use crate::{AudioChunk, AudioSource, CaptureError, Channel};

/// Everything the IO callback needs; boxed so its address is stable.
struct Ctx {
    sink: Sender<AudioChunk>,
    rate: u32,
    channels: usize,
}

struct Running {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    ctx: *mut Ctx,
    output_uid: String,
}

pub struct SystemAudioTap {
    running: Option<Running>,
    sink: Option<Sender<AudioChunk>>,
    rate: u32,
}

impl SystemAudioTap {
    pub fn new() -> Self {
        Self { running: None, sink: None, rate: 0 }
    }
}

impl Default for SystemAudioTap {
    fn default() -> Self {
        Self::new()
    }
}

fn check(status: i32, what: &str) -> Result<(), CaptureError> {
    if status == 0 { Ok(()) } else { Err(CaptureError::Device(format!("{what} failed (OSStatus {status})"))) }
}

fn address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Reads a fixed-size property.
unsafe fn get<T: Copy>(object: AudioObjectID, selector: u32, mut value: T) -> Result<T, CaptureError> {
    let mut addr = address(selector);
    let mut size = size_of::<T>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    check(status, "AudioObjectGetPropertyData")?;
    Ok(value)
}

fn default_output_uid() -> Result<String, CaptureError> {
    unsafe {
        let device: AudioObjectID =
            get(kAudioObjectSystemObject as AudioObjectID, kAudioHardwarePropertyDefaultSystemOutputDevice, 0)?;
        // A CFStringRef (toll-free bridged to NSString), owned by the caller.
        let uid: *mut NSString = get(device, kAudioDevicePropertyDeviceUID, std::ptr::null_mut())?;
        let uid = Retained::from_raw(uid).ok_or(CaptureError::NoDevice)?;
        Ok(uid.to_string())
    }
}

fn key(k: &CStr) -> Retained<NSString> {
    NSString::from_str(k.to_str().expect("Core Audio keys are ASCII"))
}

fn dict(pairs: &[(&CStr, &AnyObject)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<Retained<NSString>> = pairs.iter().map(|(k, _)| key(k)).collect();
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<&AnyObject> = pairs.iter().map(|(_, v)| *v).collect();
    NSDictionary::from_slices(&key_refs, &values)
}

/// Core Audio IO callback: downmixes the tap's float32 input to mono and sends it on.
unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    let ctx = unsafe { &*(client as *const Ctx) };
    let list = input.as_ptr();
    let n = unsafe { (*list).mNumberBuffers } as usize;
    let buffers = unsafe { std::ptr::addr_of!((*list).mBuffers) } as *const AudioBuffer;
    let mut mono: Vec<f32> = Vec::new();
    for i in 0..n {
        let b = unsafe { &*buffers.add(i) };
        if b.mData.is_null() {
            continue;
        }
        let ch = (b.mNumberChannels as usize).max(1);
        let samples =
            unsafe { std::slice::from_raw_parts(b.mData as *const f32, b.mDataByteSize as usize / size_of::<f32>()) };
        let frames = samples.len() / ch;
        if mono.is_empty() {
            mono = vec![0.0; frames];
        }
        // Non-interleaved: one buffer per channel; interleaved: channels inside one buffer.
        let total = (n * ch).max(ctx.channels).max(1) as f32;
        for (f, out) in mono.iter_mut().enumerate().take(frames) {
            *out += samples[f * ch..f * ch + ch].iter().sum::<f32>() / total;
        }
    }
    if !mono.is_empty() {
        let _ = ctx.sink.send(AudioChunk {
            channel: Channel::Others,
            sample_rate: ctx.rate,
            samples: mono,
            captured_at: Instant::now(),
        });
    }
    0
}

impl SystemAudioTap {
    fn build(&mut self, sink: Sender<AudioChunk>) -> Result<(), CaptureError> {
        let output_uid = default_output_uid()?;
        unsafe {
            let empty: Retained<NSArray<NSNumber>> = NSArray::new();
            let desc = CATapDescription::initMonoGlobalTapButExcludeProcesses(CATapDescription::alloc(), &empty);
            desc.setName(&NSString::from_str("Tyst system audio"));
            desc.setPrivate(true);
            let tap_uuid = desc.UUID().UUIDString();
            let mut tap: AudioObjectID = 0;
            check(AudioHardwareCreateProcessTap(Some(&desc), &mut tap), "AudioHardwareCreateProcessTap")?;

            let format: AudioStreamBasicDescription = match get(tap, kAudioTapPropertyFormat, std::mem::zeroed()) {
                Ok(f) => f,
                Err(e) => {
                    AudioHardwareDestroyProcessTap(tap);
                    return Err(e);
                }
            };

            let sub_device = dict(&[(kAudioSubDeviceUIDKey, &NSString::from_str(&output_uid))]);
            let sub_tap =
                dict(&[(kAudioSubTapUIDKey, &tap_uuid), (kAudioSubTapDriftCompensationKey, &NSNumber::new_bool(true))]);
            let aggregate_uid = NSUUID::new().UUIDString();
            let desc_dict = dict(&[
                (kAudioAggregateDeviceNameKey, &NSString::from_str("Tyst system audio")),
                (kAudioAggregateDeviceUIDKey, &aggregate_uid),
                (kAudioAggregateDeviceMainSubDeviceKey, &NSString::from_str(&output_uid)),
                (kAudioAggregateDeviceIsPrivateKey, &NSNumber::new_bool(true)),
                (kAudioAggregateDeviceIsStackedKey, &NSNumber::new_bool(false)),
                (kAudioAggregateDeviceTapAutoStartKey, &NSNumber::new_bool(true)),
                (kAudioAggregateDeviceSubDeviceListKey, &NSArray::from_retained_slice(&[sub_device])),
                (kAudioAggregateDeviceTapListKey, &NSArray::from_retained_slice(&[sub_tap])),
            ]);
            let cf: &CFDictionary = &*(Retained::as_ptr(&desc_dict) as *const CFDictionary);
            let mut aggregate: AudioObjectID = 0;
            if let Err(e) = check(
                AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut aggregate)),
                "AudioHardwareCreateAggregateDevice",
            ) {
                AudioHardwareDestroyProcessTap(tap);
                return Err(e);
            }

            let ctx = Box::into_raw(Box::new(Ctx {
                sink,
                rate: format.mSampleRate as u32,
                channels: format.mChannelsPerFrame as usize,
            }));
            let mut proc_id: AudioDeviceIOProcID = None;
            let created = check(
                AudioDeviceCreateIOProcID(aggregate, Some(io_proc), ctx.cast(), NonNull::from(&mut proc_id)),
                "AudioDeviceCreateIOProcID",
            )
            .and_then(|_| check(AudioDeviceStart(aggregate, proc_id), "AudioDeviceStart"));
            if let Err(e) = created {
                if proc_id.is_some() {
                    AudioDeviceDestroyIOProcID(aggregate, proc_id);
                }
                AudioHardwareDestroyAggregateDevice(aggregate);
                AudioHardwareDestroyProcessTap(tap);
                drop(Box::from_raw(ctx));
                return Err(e);
            }
            self.rate = format.mSampleRate as u32;
            log::info!("system audio tap started: {} Hz, {} channel(s)", format.mSampleRate, format.mChannelsPerFrame);
            self.running = Some(Running { tap, aggregate, proc_id, ctx, output_uid });
        }
        Ok(())
    }

    fn teardown(&mut self) {
        if let Some(r) = self.running.take() {
            unsafe {
                AudioDeviceStop(r.aggregate, r.proc_id);
                AudioDeviceDestroyIOProcID(r.aggregate, r.proc_id);
                AudioHardwareDestroyAggregateDevice(r.aggregate);
                AudioHardwareDestroyProcessTap(r.tap);
                // The IO proc is destroyed, so nothing uses the context any more.
                drop(Box::from_raw(r.ctx));
            }
        }
    }
}

impl AudioSource for SystemAudioTap {
    fn start(&mut self, sink: Sender<AudioChunk>) -> Result<(), CaptureError> {
        self.teardown();
        self.sink = Some(sink.clone());
        self.build(sink)
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        self.teardown();
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn device_name(&self) -> String {
        "Core Audio process tap (all apps)".into()
    }

    /// Follows the default output device.
    fn poll(&mut self) -> Result<(), CaptureError> {
        let Some(r) = &self.running else { return Ok(()) };
        let Ok(uid) = default_output_uid() else { return Ok(()) };
        if uid != r.output_uid {
            log::info!("default output changed, rebuilding the system audio tap");
            let sink = self.sink.clone().ok_or(CaptureError::NoDevice)?;
            self.teardown();
            self.build(sink)?;
        }
        Ok(())
    }
}

impl Drop for SystemAudioTap {
    fn drop(&mut self) {
        self.teardown();
    }
}
