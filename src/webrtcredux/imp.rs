use std::future::Future;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use futures::future;
use futures::prelude::*;
use gst::{debug, error, ErrorMessage, glib, info, prelude::PadExtManual, trace, traits::{ElementExt, GstObjectExt}};
use gst_base::subclass::prelude::*;
use interceptor::registry::Registry;
use once_cell::sync::Lazy;
use strum_macros::EnumString;
use tokio::runtime;
use webrtc::api::{API, APIBuilder};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
pub use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        "webrtcredux",
        gst::DebugColorFlags::empty(),
        Some("WebRTC Video and Audio Transmitter"),
    )
});

static RUNTIME: Lazy<runtime::Runtime> = Lazy::new(|| {
    runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
        .unwrap()
});


#[derive(Debug, PartialEq, EnumString)]
enum MediaType {
    #[strum(ascii_case_insensitive, serialize = "video/H264", serialize = "video/x-h264")]
    H264,
    #[strum(ascii_case_insensitive, serialize = "video/VP8")]
    VP8,
    #[strum(ascii_case_insensitive, serialize = "video/VP9")]
    VP9,
    #[strum(ascii_case_insensitive, serialize = "audio/opus", serialize = "audio/x-opus")]
    Opus,
    #[strum(ascii_case_insensitive, serialize = "audio/G722")]
    G722,
    #[strum(ascii_case_insensitive, serialize = "audio/PCMU", serialize = "audio/x-mulaw")]
    Mulaw,
    #[strum(ascii_case_insensitive, serialize = "audio/PCMA", serialize = "audio/alaw")]
    Alaw,
}

enum MediaState {
    NotConfigured,
    Configured { media_type: MediaType },
}

struct WebRtcState {
    api: API,
    config: Option<RTCConfiguration>,
}

struct State {
    video_state: Option<MediaState>,
    audio_state: Option<MediaState>,
    webrtc_state: Mutex<Option<WebRtcState>>,
}

struct GenericSettings {
    timeout: u16,
}

impl Default for GenericSettings {
    fn default() -> Self {
        Self {
            timeout: 15,
        }
    }
}

#[derive(Default)]
pub struct WebRtcRedux {
    state: Arc<Mutex<Option<State>>>,
    settings: Arc<Mutex<GenericSettings>>,
    canceller: Mutex<Option<future::AbortHandle>>,
}

impl WebRtcRedux {
    pub fn add_ice_servers(&self, mut ice_server: Vec<RTCIceServer>) {
        let mut state_lock = self.state.lock().unwrap();
        let state = state_lock.as_mut().unwrap();
        let mut webrtc_state_lock = state.webrtc_state.lock().unwrap();
        let mut webrtc_state = webrtc_state_lock.as_mut().unwrap();

        match webrtc_state.config {
            Some(ref mut config) => {
                config.ice_servers.append(&mut ice_server);
            }
            None => {
                error!(CAT, "Trying to add ice servers after starting");
            }
        }
    }

    fn wait<F, T>(&self, future: F) -> Result<T, Option<gst::ErrorMessage>>
        where
            F: Send + Future<Output=Result<T, gst::ErrorMessage>>,
            T: Send + 'static,
    {
        let timeout = self.settings.lock().unwrap().timeout;

        let mut canceller = self.canceller.lock().unwrap();
        let (abort_handle, abort_registration) = future::AbortHandle::new_pair();
        canceller.replace(abort_handle);
        drop(canceller);

        // Wrap in a timeout
        let future = async {
            if timeout == 0 {
                future.await
            } else {
                let res = tokio::time::timeout(std::time::Duration::from_secs(timeout.into()), future).await;

                match res {
                    Ok(res) => res,
                    Err(_) => Err(gst::error_msg!(
                        gst::ResourceError::Read,
                        ["Request timeout"]
                    )),
                }
            }
        };

        // And make abortable
        let future = async {
            match future::Abortable::new(future, abort_registration).await {
                Ok(res) => res.map_err(Some),
                Err(_) => Err(None),
            }
        };

        let res = {
            let _enter = RUNTIME.enter();
            futures::executor::block_on(future)
        };

        /* Clear out the canceller */
        let _ = self.canceller.lock().unwrap().take();

        res
    }
}

#[glib::object_subclass]
impl ObjectSubclass for WebRtcRedux {
    const NAME: &'static str = "WebRtcRedux";
    type Type = super::WebRtcRedux;
    type ParentType = gst_base::BaseSink;

    fn with_class(_klass: &Self::Class) -> Self {
        let mut media_engine = MediaEngine::default();
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine).expect("Failed to register default interceptors");
        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();

        let webrtc_state = Mutex::new(Some(WebRtcState {
            api,
            config: Some(RTCConfiguration::default()),
        }));

        Self { state: Arc::new(Mutex::new(Some(State { video_state: None, audio_state: None, webrtc_state }))), ..Default::default() }
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
                            state.lock().unwrap().as_mut().unwrap().video_state = Some(MediaState::Configured { media_type: MediaType::from_str(mime).expect("Failed to parse mime type") });
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
                            state.lock().unwrap().as_mut().unwrap().audio_state = Some(MediaState::Configured { media_type: MediaType::from_str(mime).expect("Failed to parse mime type") });
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
    fn start(&self, _sink: &Self::Type) -> Result<(), gst::ErrorMessage> {
        let mut state_lock = self.state.lock().unwrap();
        let state = state_lock.as_mut().unwrap();
        let mut webrtc_state_lock = state.webrtc_state.lock().unwrap();
        let mut webrtc_state = webrtc_state_lock.as_mut().unwrap();

        let peer_connection = match webrtc_state.config.take() {
            Some(config) => {
                let future = async {
                    webrtc_state.api.new_peer_connection(config).await.map_err(|e| {
                        gst::error_msg!(
                            gst::ResourceError::Failed,
                            ["Failed to create PeerConnection: {:?}", e]
                        )
                    })
                };

                self.wait(future)
            }
            None => {
                return Err(gst::error_msg!(gst::LibraryError::Settings, ["WebRTC configuration not set"]));
            }
        };

        info!(CAT, "Started");
        Ok(())
    }

    fn stop(&self, _sink: &Self::Type) -> Result<(), gst::ErrorMessage> {
        //Drop state
        self.state.lock().unwrap().take();
        info!(CAT, "Stopped");
        Ok(())
    }

    fn unlock(&self, _sink: &Self::Type) -> Result<(), ErrorMessage> {
        let canceller = self.canceller.lock().unwrap();
        if let Some(ref canceller) = *canceller {
            canceller.abort();
        }
        Ok(())
    }
}