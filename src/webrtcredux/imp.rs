use std::sync::{Arc, Mutex};

use gst::{debug, error, glib, info, prelude::PadExtManual, trace, traits::{ElementExt, GstObjectExt}};

use gst_base::subclass::prelude::*;
use interceptor::registry::Registry;

use once_cell::sync::Lazy;
use webrtc::api::media_engine::MediaEngine;
pub use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;

use crate::glib::{ParamSpec, StaticType, ToValue, Value};
use crate::glib::subclass::Signal;

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
    Opus,
    G722,
    Mulaw,
    Alaw,
}

impl MediaType {
    fn from_mime(mime: &str) -> MediaType {
        match mime {
            "video/x-h264" => MediaType::H264,
            "video/VP8" => MediaType::VP8,
            "video/VP9" => MediaType::VP9,
            "audio/x-opus" => MediaType::Opus,
            "audio/G722" => MediaType::G722,
            "audio/x-mulaw" => MediaType::Mulaw,
            "audio/alaw" => MediaType::Alaw,
            _ => unreachable!("Something's very wrong!")
        }
    }
}


enum MediaState {
    NotConfigured,
    Configured { media_type: MediaType },
}

struct WebRtcState {
    media_engine: MediaEngine,
    registry: Registry,
    config: RTCConfiguration,
}

struct State {
    video_state: Option<MediaState>,
    audio_state: Option<MediaState>,
    webrtc_state: Mutex<Option<WebRtcState>>,
}

#[derive(Default)]
pub struct WebRtcRedux {
    state: Arc<Mutex<Option<State>>>,
}

impl WebRtcRedux {
    pub fn add_ice_servers(&self, mut ice_server: Vec<RTCIceServer>) {
        let mut state_lock = self.state.lock().unwrap();
        let state = state_lock.as_mut().unwrap();
        let mut webrtc_state_lock = state.webrtc_state.lock().unwrap();
        let mut webrtc_state = webrtc_state_lock.as_mut().unwrap();

        webrtc_state.config.ice_servers.append(&mut ice_server);
    }
}

#[glib::object_subclass]
impl ObjectSubclass for WebRtcRedux {
    const NAME: &'static str = "WebRtcRedux";
    type Type = super::WebRtcRedux;
    type ParentType = gst_base::BaseSink;

    fn with_class(_klass: &Self::Class) -> Self {
        let webrtc_state = Mutex::new(Some(WebRtcState {
            media_engine: Default::default(),
            registry: Default::default(),
            config: Default::default()
        }));

        Self { state: Arc::new(Mutex::new(Some(State { video_state: None, audio_state: None, webrtc_state }))) }
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
                "Jack Hogan",
            )
        });

        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: Lazy<Vec<gst::PadTemplate>> = Lazy::new(|| {
            //MIME_TYPE_H264
            let mut video_caps = gst::Caps::new_empty_simple("video/x-h264");
            let video_caps = video_caps.get_mut().unwrap();
            //MIME_TYPE_VP8
            video_caps.append(gst::Caps::new_empty_simple("video/VP8"));
            //MIME_TYPE_VP9
            video_caps.append(gst::Caps::new_empty_simple("video/VP9"));

            let video_sink = gst::PadTemplate::new(
                "video",
                gst::PadDirection::Sink,
                gst::PadPresence::Request,
                &video_caps.to_owned(),
            ).unwrap();

            //MIME_TYPE_OPUS
            let mut audio_caps = gst::Caps::new_empty_simple("audio/x-opus");
            let audio_caps = audio_caps.get_mut().unwrap();
            //MIME_TYPE_G722
            audio_caps.append(gst::Caps::new_empty_simple("audio/G722"));
            //MIME_TYPE_PCMU
            audio_caps.append(gst::Caps::new_empty_simple("audio/x-mulaw"));
            //MIME_TYPE_PCMA
            audio_caps.append(gst::Caps::new_empty_simple("audio/alaw"));

            let audio_sink = gst::PadTemplate::new(
                "audio",
                gst::PadDirection::Sink,
                gst::PadPresence::Request,
                &audio_caps.to_owned(),
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
                    pad.set_event_function(move |_pad, _parent, event| {
                        let structure = event.structure().unwrap();
                        if structure.name() == "GstEventCaps" {
                            let mime = structure.get::<gst::Caps>("caps").unwrap().structure(0).unwrap().name();
                            state.lock().unwrap().as_mut().unwrap().video_state = Some(MediaState::Configured { media_type: MediaType::from_mime(mime) });
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
            }
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
                    pad.set_event_function(move |_pad, _parent, event| {
                        let structure = event.structure().unwrap();
                        if structure.name() == "GstEventCaps" {
                            let mime = structure.get::<gst::Caps>("caps").unwrap().structure(0).unwrap().name();
                            state.lock().unwrap().as_mut().unwrap().audio_state = Some(MediaState::Configured { media_type: MediaType::from_mime(mime) });
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
            }
            _ => {
                debug!(CAT, obj: element, "Requested pad is not audio or video");
                None
            }
        }
    }

    fn release_pad(&self, element: &Self::Type, pad: &gst::Pad) {
        if pad.name() == "video" {
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
        //Drop state
        self.state.lock().unwrap().take();
        info!(CAT, "Stopped");
        Ok(())
    }
}