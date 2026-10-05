//! macOS system audio through a Core Audio process tap (SPEC 10.1, macOS 14.4+): a private,
//! global, mono tap of every process's output, inside a private aggregate device whose clock is
//! the default output device. Needs `NSAudioCaptureUsageDescription`; macOS asks the user once.
//!
//! With an app list ([`SystemAudioTap::only_apps`], SPEC 15 q7) the tap mixes only the processes
//! whose bundle id matches the list, so music and notification sounds stay out of Others. Apps
//! that start or quit while recording are picked up by the next [`SystemAudioTap::poll`]; while
//! none runs, Others is silent.
//!
//! When the default output changes (headphones plugged in), or its sample rate does (Bluetooth
//! headphones drop to 16 or 24 kHz while their microphone is in use), [`SystemAudioTap::poll`]
//! rebuilds the aggregate device, so the session keeps running.

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
    AudioHardwareDestroyProcessTap, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, CATapDescription, kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey, kAudioAggregateDeviceSubDeviceListKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey,
    kAudioDevicePropertyDeviceUID, kAudioDevicePropertyNominalSampleRate,
    kAudioHardwarePropertyDefaultSystemOutputDevice, kAudioHardwarePropertyProcessObjectList,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    kAudioProcessPropertyBundleID, kAudioProcessPropertyPID, kAudioSubDeviceUIDKey, kAudioSubTapDriftCompensationKey,
    kAudioSubTapUIDKey, kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};

use crate::mic_watch::app_matches;
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
    rate: u32,
}

pub struct SystemAudioTap {
    running: Option<Running>,
    sink: Option<Sender<AudioChunk>>,
    rate: u32,
    /// Only these apps (`None`: every app).
    apps: Option<Vec<String>>,
    /// The process objects in the current tap (with an app list), sorted.
    processes: Vec<AudioObjectID>,
}

impl SystemAudioTap {
    /// Every app's output.
    pub fn new() -> Self {
        Self { running: None, sink: None, rate: 0, apps: None, processes: Vec::new() }
    }

    /// Only the output of apps matching `apps` (part of the bundle id, as in meeting detection).
    pub fn only_apps(apps: Vec<String>) -> Self {
        Self { running: None, sink: None, rate: 0, apps: Some(apps), processes: Vec::new() }
    }
}

impl Default for SystemAudioTap {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn check(status: i32, what: &str) -> Result<(), CaptureError> {
    if status == 0 { Ok(()) } else { Err(CaptureError::Device(format!("{what} failed (OSStatus {status})"))) }
}

pub(crate) fn address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Reads a fixed-size property.
pub(crate) unsafe fn get<T: Copy>(object: AudioObjectID, selector: u32, mut value: T) -> Result<T, CaptureError> {
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

/// Reads a variable-length array property.
pub(crate) unsafe fn get_array<T: Copy + Default>(
    object: AudioObjectID,
    selector: u32,
) -> Result<Vec<T>, CaptureError> {
    let mut addr = address(selector);
    let mut size = 0u32;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(object, NonNull::from(&mut addr), 0, std::ptr::null(), NonNull::from(&mut size))
    };
    check(status, "AudioObjectGetPropertyDataSize")?;
    let mut values = vec![T::default(); size as usize / size_of::<T>()];
    if values.is_empty() {
        return Ok(values);
    }
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(values.as_mut_ptr()).expect("non-empty vec").cast(),
        )
    };
    check(status, "AudioObjectGetPropertyData")?;
    values.truncate(size as usize / size_of::<T>());
    Ok(values)
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

/// The rate the aggregate device runs at, which is the rate of the frames the IO proc gets. It
/// follows the main sub-device (the output), not the tap's own format: AirPods in headset mode
/// run at 24 kHz while the tap still reports 48 kHz.
fn nominal_rate(device: AudioObjectID) -> Option<u32> {
    let rate: f64 = unsafe { get(device, kAudioDevicePropertyNominalSampleRate, 0.0) }.ok()?;
    (rate >= 1.0).then_some(rate.round() as u32)
}

/// A Core Audio process object's bundle id.
pub(crate) fn bundle_id(object: AudioObjectID) -> Option<String> {
    unsafe {
        let bundle: *mut NSString = get(object, kAudioProcessPropertyBundleID, std::ptr::null_mut()).ok()?;
        Retained::from_raw(bundle).map(|s| s.to_string())
    }
}

/// Process objects (other than Tyst) whose bundle id matches `apps`, sorted.
fn matching_processes(apps: &[String]) -> Vec<AudioObjectID> {
    let own = std::process::id() as i32;
    let objects: Vec<AudioObjectID> = match unsafe {
        get_array(kAudioObjectSystemObject as AudioObjectID, kAudioHardwarePropertyProcessObjectList)
    } {
        Ok(o) => o,
        Err(e) => {
            log::warn!("audio processes: {e}");
            return Vec::new();
        }
    };
    let mut found: Vec<AudioObjectID> = objects
        .into_iter()
        .filter(|&o| unsafe { get(o, kAudioProcessPropertyPID, -1i32) }.unwrap_or(-1) != own)
        .filter(|&o| bundle_id(o).is_some_and(|b| app_matches(&b, apps)))
        .collect();
    found.sort_unstable();
    found
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
        if let Some(apps) = &self.apps {
            self.processes = matching_processes(apps);
            if self.processes.is_empty() {
                log::info!("system audio: no meeting app is running, Others waits for one");
                return Ok(());
            }
            let names: Vec<String> = self.processes.iter().filter_map(|&p| bundle_id(p)).collect();
            log::info!("system audio: only meeting apps: {}", names.join(", "));
        }
        unsafe {
            let desc = if self.apps.is_some() {
                let ids: Vec<Retained<NSNumber>> = self.processes.iter().map(|&p| NSNumber::new_u32(p)).collect();
                CATapDescription::initMonoMixdownOfProcesses(
                    CATapDescription::alloc(),
                    &NSArray::from_retained_slice(&ids),
                )
            } else {
                let empty: Retained<NSArray<NSNumber>> = NSArray::new();
                CATapDescription::initMonoGlobalTapButExcludeProcesses(CATapDescription::alloc(), &empty)
            };
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

            let tap_rate = format.mSampleRate as u32;
            let rate = nominal_rate(aggregate).unwrap_or(tap_rate);
            let ctx = Box::into_raw(Box::new(Ctx { sink, rate, channels: format.mChannelsPerFrame as usize }));
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
            self.rate = rate;
            log::info!(
                "system audio tap started: {rate} Hz (tap format {tap_rate} Hz), {} channel(s)",
                format.mChannelsPerFrame
            );
            self.running = Some(Running { tap, aggregate, proc_id, ctx, output_uid, rate });
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
        self.sink = None;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn device_name(&self) -> String {
        if self.apps.is_some() {
            "Core Audio process tap (meeting apps)".into()
        } else {
            "Core Audio process tap (all apps)".into()
        }
    }

    /// Follows the default output device and its sample rate, and (with an app list) the meeting
    /// apps that start or quit.
    fn poll(&mut self) -> Result<(), CaptureError> {
        if let Some(apps) = &self.apps
            && self.sink.is_some()
            && matching_processes(apps) != self.processes
        {
            log::info!("meeting apps changed, rebuilding the system audio tap");
            let sink = self.sink.clone().ok_or(CaptureError::NoDevice)?;
            self.teardown();
            return self.build(sink);
        }
        let Some(r) = &self.running else { return Ok(()) };
        let Ok(uid) = default_output_uid() else { return Ok(()) };
        let rate_changed = nominal_rate(r.aggregate).is_some_and(|rate| rate != r.rate);
        if uid != r.output_uid || rate_changed {
            log::info!(
                "default output {}, rebuilding the system audio tap",
                if uid != r.output_uid { "changed" } else { "changed its sample rate" }
            );
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
