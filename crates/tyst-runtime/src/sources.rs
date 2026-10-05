//! Capture sources for this platform (SPEC 6.1), shared by the app and the CLI so both record
//! the same way: Me is the default microphone (PipeWire on Linux, cpal elsewhere), Others is
//! system audio (PipeWire sink monitor on Linux, Core Audio process tap on macOS).

use tyst_core::transcript::Channel;

use crate::meeting::SourceFactory;

/// The microphone source, where this build has one.
pub fn microphone() -> Option<SourceFactory> {
    #[cfg(all(target_os = "linux", feature = "pipewire"))]
    return Some(Box::new(|| Ok(Box::new(tyst_platform::pipewire::PipeWireSource::microphone()) as _)));
    #[cfg(all(not(all(target_os = "linux", feature = "pipewire")), feature = "mic"))]
    return Some(Box::new(|| Ok(Box::new(tyst_platform::mic::MicSource::new()) as _)));
    #[allow(unreachable_code)]
    None
}

/// The system-audio source, where this build has one.
pub fn system_audio() -> Option<SourceFactory> {
    #[cfg(all(target_os = "linux", feature = "pipewire"))]
    return Some(Box::new(|| Ok(Box::new(tyst_platform::pipewire::PipeWireSource::system_audio()) as _)));
    #[cfg(all(target_os = "macos", feature = "macos-tap"))]
    return Some(Box::new(|| Ok(Box::new(tyst_platform::macos_tap::SystemAudioTap::new()) as _)));
    #[allow(unreachable_code)]
    None
}

/// System audio from the apps in `apps` only (SPEC 15 q7), where this build can filter by app
/// (macOS); elsewhere every app, like [`system_audio`].
pub fn meeting_apps_audio(apps: Vec<String>) -> Option<SourceFactory> {
    #[cfg(all(target_os = "macos", feature = "macos-tap"))]
    return Some(Box::new(move || Ok(Box::new(tyst_platform::macos_tap::SystemAudioTap::only_apps(apps)) as _)));
    #[allow(unreachable_code)]
    {
        let _ = apps;
        system_audio()
    }
}

/// Whether [`meeting_apps_audio`] really filters on this build.
pub const FILTERS_APPS: bool = cfg!(all(target_os = "macos", feature = "macos-tap"));

pub fn source(channel: Channel) -> Option<SourceFactory> {
    match channel {
        Channel::Me => microphone(),
        Channel::Others => system_audio(),
    }
}
