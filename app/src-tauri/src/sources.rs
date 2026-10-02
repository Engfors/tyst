//! Capture sources for this platform (SPEC 6.1): Me is the default microphone, Others is system
//! audio.

use tyst_core::transcript::Channel;
use tyst_runtime::meeting::SourceFactory;

/// The microphone source.
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
    #[allow(unreachable_code)]
    None
}

pub fn source(channel: Channel) -> Option<SourceFactory> {
    match channel {
        Channel::Me => microphone(),
        Channel::Others => system_audio(),
    }
}
