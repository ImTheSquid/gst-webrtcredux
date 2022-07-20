use gst::{glib, info, debug, traits::{ElementExt, GstObjectExt}, prelude::PadExtManual, trace};
use gst_base::subclass::prelude::*;
use std::sync::{Arc, Mutex};

use once_cell::sync::Lazy;

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        "webrtcredux",
        gst::DebugColorFlags::empty(),
        Some("WebRTC Video and Audio Transmitter"),
    )
});

enum MediaType {
    H264,
    VP8,
    VP9,
    OPUS,
    G722,
    MULAW,
    ALAW
}

impl MediaType {
    fn from_mime(mime: &str) -> MediaType {
        match mime {
            "video/x-h264" => MediaType::H264,
            "video/VP8" => MediaType::VP8,
            "video/VP9" => MediaType::VP9,
            "audio/x-opus" => MediaType::OPUS,
            "audio/G722" => MediaType::G722,
            "audio/x-mulaw" => MediaType::MULAW,
            "audio/alaw" => MediaType::ALAW,
            _ => unreachable!("Something's very wrong!")
        }
    }
}

enum MediaState {
    NotConfigured,
    Configured { media_type: MediaType }
}

struct State {
    video_state: Option<MediaState>,
    audio_state: Option<MediaState>
}

#[derive(Default)]
pub struct WebRtcRedux {
    state: Arc<Mutex<Option<State>>>
}

impl WebRtcRedux {}

#[glib::object_subclass]
impl ObjectSubclass for WebRtcRedux {
    const NAME: &'static str = "WebRtcRedux";
    type Type = super::WebRtcRedux;
    type ParentType = gst_base::BaseSink;

    fn with_class(_klass: &Self::Class) -> Self {
        Self { state: Arc::new(Mutex::new(Some(State{ video_state: None, audio_state: None }))) }
    }
}

impl ObjectImpl for WebRtcRedux {}
impl ElementImpl for WebRtcRedux {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: Lazy<gst::subclass::ElementMetadata> = Lazy::new(|| {
            gst::subclass::ElementMetadata::new(
                "WebRTC Broadcast Engine",
            "Sink/Video/Audio",
            "Broadcasts encoded video and audio",
            "Jack Hogan"
            )
        });

        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: Lazy<Vec<gst::PadTemplate>> = Lazy::new(|| {
            let mut video_caps = gst::Caps::new_empty_simple("video/x-h264");
            let video_caps = video_caps.get_mut().unwrap();
            video_caps.append(gst::Caps::new_empty_simple("video/VP8"));
            video_caps.append(gst::Caps::new_empty_simple("video/VP9"));

            let video_sink = gst::PadTemplate::new(
                "video", 
                gst::PadDirection::Sink, 
                gst::PadPresence::Request,
                &video_caps.to_owned()
            ).unwrap();

            let mut audio_caps = gst::Caps::new_empty_simple("audio/x-opus");
            let audio_caps = audio_caps.get_mut().unwrap();
            audio_caps.append(gst::Caps::new_empty_simple("audio/G722"));
            audio_caps.append(gst::Caps::new_empty_simple("audio/x-mulaw"));
            audio_caps.append(gst::Caps::new_empty_simple("audio/alaw"));

            let audio_sink = gst::PadTemplate::new(
                "audio", 
                gst::PadDirection::Sink, 
                gst::PadPresence::Request,
                &audio_caps.to_owned()
            ).unwrap();

            vec![video_sink, audio_sink]
        });

        PAD_TEMPLATES.as_ref()
    }

    fn request_new_pad(&self, element: &Self::Type, templ: &gst::PadTemplate, _name: Option<String>, _caps: Option<&gst::Caps>) -> Option<gst::Pad> {
        let mut state = self.state.lock().unwrap();
        let state = state.as_mut().unwrap();

        // Set up audio and video pads along with callbacks for events and data
        match templ.name_template() {
            "video" => {
                if state.video_state.is_some() {
                    debug!(
                        CAT,
                        obj: element,
                        "requested_new_pad: video pad is already set"
                    );
                    return None;
                }

                let pad = gst::Pad::from_template(templ, Some("video"));
                unsafe {
                    let state = self.state.clone();
                    pad.set_event_function(move|_pad, _parent, event| {
                        let structure = event.structure().unwrap();
                        if structure.name() == "GstEventCaps" {
                            let mime = structure.get::<gst::Caps>("caps").unwrap().structure(0).unwrap().name();
                            state.lock().unwrap().as_mut().unwrap().video_state = Some(MediaState::Configured { media_type: MediaType::from_mime(mime)});
                            debug!(CAT, "Video media type set to: {}", mime);
                        }
                        true
                    });

                    pad.set_chain_function(|_pad, _parent, buffer| {
                        let map = buffer.map_readable().map_err(|_| {
                            gst::FlowError::Error
                        })?;
                        trace!(CAT, "Video map Size: {}", map.size());
                        Ok(gst::FlowSuccess::Ok)
                    });
                }
                element.add_pad(&pad).unwrap();

                state.video_state = Some(MediaState::NotConfigured);

                Some(pad)
            },
            "audio" => {
                if state.audio_state.is_some() {
                    debug!(
                        CAT,
                        obj: element,
                        "requested_new_pad: audio pad is already set"
                    );
                    return None;
                }

                let pad = gst::Pad::from_template(templ, Some("audio"));
                unsafe {
                    let state = self.state.clone();
                    pad.set_event_function(move|_pad, _parent, event| {
                        let structure = event.structure().unwrap();
                        if structure.name() == "GstEventCaps" {
                            let mime = structure.get::<gst::Caps>("caps").unwrap().structure(0).unwrap().name();
                            state.lock().unwrap().as_mut().unwrap().audio_state = Some(MediaState::Configured { media_type: MediaType::from_mime(mime)});
                            debug!(CAT, "Audio media type set to: {}", mime);
                        }
                        true
                    });

                    pad.set_chain_function(|_pad, _parent, buffer| {
                        let map = buffer.map_readable().map_err(|_| {
                            gst::FlowError::Error
                        })?;
                        trace!(CAT, "Audio map Size: {}", map.size());
                        Ok(gst::FlowSuccess::Ok)
                    });
                }
                element.add_pad(&pad).unwrap();

                state.audio_state = Some(MediaState::NotConfigured);

                Some(pad)
            },
            _ => {
                debug!(CAT, obj: element, "Requested pad is not audio or video");
                None
            }
        }
    }

    fn release_pad(&self, element: &Self::Type, pad: &gst::Pad) {
        let _ = if pad.name() == "video" {
            self.state.lock().unwrap().as_mut().unwrap().video_state.take();
        } else {
            self.state.lock().unwrap().as_mut().unwrap().audio_state.take();
        };

        self.parent_release_pad(element, pad);
    }

    fn provide_clock(&self, _element: &Self::Type) -> Option<gst::Clock> {
        Some(gst::SystemClock::obtain())
    }
}

impl GstObjectImpl for WebRtcRedux {}

impl BaseSinkImpl for WebRtcRedux {
    fn start(&self, _element: &Self::Type) -> Result<(), gst::ErrorMessage> {
        info!(CAT, "Started");
        Ok(())
    }

    fn stop(&self, _element: &Self::Type) -> Result<(), gst::ErrorMessage> {
        info!(CAT, "Stopped");
        Ok(())
    }
}