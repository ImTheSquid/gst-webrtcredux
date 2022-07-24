use std::collections::HashMap;
use std::future::Future;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use futures::future;
use gst::prelude::ObjectExt;
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
use webrtc::peer_connection::RTCPeerConnection;

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


#[derive(Debug, PartialEq, Eq, EnumString, Clone, Copy)]
enum MediaType {
    #[strum(ascii_case_insensitive, serialize = "video/H264", serialize = "video/x-h264")]
    H264,
    #[strum(ascii_case_insensitive, serialize = "video/x-vp8")]
    VP8,
    #[strum(ascii_case_insensitive, serialize = "video/x-vp9")]
    VP9,
    #[strum(ascii_case_insensitive, serialize = "audio/opus", serialize = "audio/x-opus")]
    Opus,
    #[strum(ascii_case_insensitive, serialize = "audio/G722")]
    G722,
    #[strum(ascii_case_insensitive, serialize = "audio/PCMU", serialize = "audio/x-mulaw")]
    Mulaw,
    #[strum(ascii_case_insensitive, serialize = "audio/PCMA", serialize = "audio/x-alaw")]
    Alaw,
}

#[derive(Clone, PartialEq, Eq)]
enum MediaState {
    NotConfigured,
    TypeConfigured(MediaType),
    IdConfigured(String),
    Configured { media: MediaType, id: String }
}

impl MediaState {
    fn add_id(&self, new_id: &str) -> Self {
        match self {
            MediaState::NotConfigured => MediaState::IdConfigured(new_id.to_string()),
            MediaState::TypeConfigured(media) => MediaState::Configured { media: *media, id: new_id.to_string() },
            MediaState::IdConfigured(_) => MediaState::IdConfigured(new_id.to_string()),
            MediaState::Configured { media, id: _ } => MediaState::Configured { media: *media, id: new_id.to_string() },
        }
    }

    fn add_media(&self, new_media: MediaType) -> Self {
        match self {
            MediaState::NotConfigured => MediaState::TypeConfigured(new_media),
            MediaState::TypeConfigured(_) => MediaState::TypeConfigured(new_media),
            MediaState::IdConfigured(id) => MediaState::Configured { media: new_media, id: id.to_owned() },
            MediaState::Configured { media: _, id } => MediaState::Configured { media: new_media, id: id.to_owned() },
        }
    }
}

struct WebRtcState {
    api: API,
    peer_connection: Option<RTCPeerConnection>,
}

#[derive(Default)]
struct State {
    video_state: HashMap<usize, MediaState>,
    next_video_pad_id: usize,
    audio_state: HashMap<usize, MediaState>,
    next_audio_pad_id: usize
}

struct WebRtcSettings {
    config: Option<RTCConfiguration>
}

impl Default for WebRtcSettings {
    fn default() -> Self {
        WebRtcSettings {
            config: Some(RTCConfiguration::default())
        }
    }
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
    webrtc_state: Arc<Mutex<Option<WebRtcState>>>,
    settings: Arc<Mutex<GenericSettings>>,
    webrtc_settings: Arc<Mutex<WebRtcSettings>>,
    canceller: Mutex<Option<future::AbortHandle>>
}

impl WebRtcRedux {
    pub fn add_ice_servers(&self, mut ice_server: Vec<RTCIceServer>) {
        let mut webrtc_settings = self.webrtc_settings.lock().unwrap();

        match webrtc_settings.config {
            Some(ref mut config) => {
                config.ice_servers.append(&mut ice_server);
            }
            None => {
                error!(CAT, "Trying to add ice servers after starting");
            }
        }
    }

    fn wait<F, T>(&self, future: F) -> Result<T, Option<ErrorMessage>>
        where
            F: Send + Future<Output=Result<T, ErrorMessage>>,
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

    pub fn set_stream_id(&self, pad_name: &str, stream_id: &str) -> Result<(), ErrorMessage> {
        let split = pad_name.split("_").collect::<Vec<_>>();
        if split.len() != 2 {
            return Err(gst::error_msg!(gst::ResourceError::NotFound, [&format!("Pad with name '{}' is invalid", pad_name)]));
        }

        let id: usize = match split[1].parse() {
            Ok(val) => val,
            Err(_) => return Err(gst::error_msg!(gst::ResourceError::NotFound, [&format!("Couldn't parse '{}' into number", split[1])]))
        };

        match split[0] {
            "video" => {
                if !self.state.lock().unwrap().as_ref().unwrap().video_state.contains_key(&id) {
                    return Err(gst::error_msg!(gst::ResourceError::NotFound, [&format!("Invalid ID: {}", id)]));
                }

                let current = {
                    let state = self.state.lock().unwrap();
                    state.as_ref().unwrap().video_state.get(&id).unwrap().to_owned()
                };

                self.state.lock().unwrap().as_mut().unwrap().video_state.insert(id, current.add_id(stream_id));

                Ok(())
            },
            "audio" => {
                if !self.state.lock().unwrap().as_ref().unwrap().audio_state.contains_key(&id) {
                    return Err(gst::error_msg!(gst::ResourceError::NotFound, [&format!("Invalid ID: {}", id)]));
                }

                let current = {
                    let state = self.state.lock().unwrap();
                    state.as_ref().unwrap().audio_state.get(&id).unwrap().to_owned()
                };

                self.state.lock().unwrap().as_mut().unwrap().audio_state.insert(id, current.add_id(stream_id));

                Ok(())
            },
            _ => Err(gst::error_msg!(gst::ResourceError::NotFound, [&format!("Pad with type '{}' not found", split[0])]))
        }
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

        let webrtc_state = Arc::new(Mutex::new(Some(WebRtcState {
            api,
            peer_connection: None,
        })));

        let state = Arc::new(Mutex::new(Some(State::default())));

        let settings = Arc::new(Mutex::new(Default::default()));

        let webrtc_settings = Arc::new(Mutex::new(Default::default()));


        Self { state, webrtc_state, settings, webrtc_settings, canceller: Mutex::new(None) }
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
            video_caps.append(gst::Caps::new_empty_simple("video/x-vp8"));
            //MIME_TYPE_VP9
            video_caps.append(gst::Caps::new_empty_simple("video/x-vp9"));

            let video_sink = gst::PadTemplate::new(
                "video_%u",
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
            audio_caps.append(gst::Caps::new_empty_simple("audio/x-alaw"));

            let audio_sink = gst::PadTemplate::new(
                "audio_%u",
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
            "video_%u" => {
                let pad = gst::Pad::from_template(templ, Some(&format!("video_{}", state.next_video_pad_id)));
                unsafe {
                    let id = state.next_video_pad_id;
                    let state = self.state.clone();
                    pad.set_event_function(move |_pad, _parent, event| {
                        let structure = event.structure().unwrap();
                        if structure.name() == "GstEventCaps" {
                            let mime = structure.get::<gst::Caps>("caps").unwrap().structure(0).unwrap().name();

                            let current = {
                                let state = state.lock().unwrap();
                                state.as_ref().unwrap().video_state.get(&id).unwrap().to_owned()
                            };
                            state.lock().unwrap().as_mut().unwrap().video_state.insert(id, current.add_media(MediaType::from_str(mime).expect("Failed to parse mime type")));

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

                state.video_state.insert(state.next_video_pad_id, MediaState::NotConfigured);
                state.next_video_pad_id += 1;

                Some(pad)
            }
            "audio_%u" => {
                let pad = gst::Pad::from_template(templ, Some(&format!("audio_{}", state.next_audio_pad_id)));
                unsafe {
                    let id = state.next_audio_pad_id;
                    let state = self.state.clone();
                    pad.set_event_function(move |_pad, _parent, event| {
                        let structure = event.structure().unwrap();
                        if structure.name() == "GstEventCaps" {
                            let mime = structure.get::<gst::Caps>("caps").unwrap().structure(0).unwrap().name();
                            
                            let current = {
                                let state = state.lock().unwrap();
                                state.as_ref().unwrap().audio_state.get(&id).unwrap().to_owned()
                            };
                            state.lock().unwrap().as_mut().unwrap().audio_state.insert(id, current.add_media(MediaType::from_str(mime).expect("Failed to parse mime type")));

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

                state.audio_state.insert(state.next_audio_pad_id, MediaState::NotConfigured);
                state.next_audio_pad_id += 1;

                Some(pad)
            }
            _ => {
                debug!(CAT, obj: element, "Requested pad is not audio or video");
                None
            }
        }
    }

    fn release_pad(&self, element: &Self::Type, pad: &gst::Pad) {
        let name = pad.name();
        let split = name.split("_").collect::<Vec<_>>();
        let id: usize = split[1].parse().unwrap();
        if split[0] == "video" {
            self.state.lock().unwrap().as_mut().unwrap().video_state.remove(&id);
        } else {
            self.state.lock().unwrap().as_mut().unwrap().audio_state.remove(&id);
        };

        self.parent_release_pad(element, pad);
    }

    fn provide_clock(&self, _element: &Self::Type) -> Option<gst::Clock> {
        Some(gst::SystemClock::obtain())
    }
}

impl GstObjectImpl for WebRtcRedux {}

impl BaseSinkImpl for WebRtcRedux {
    fn start(&self, _sink: &Self::Type) -> Result<(), ErrorMessage> {
        // Make sure all pads are configured with a stream ID
        let audio_ok = self.state.lock().unwrap().as_ref().unwrap().audio_state.values().all(|val| match *val { MediaState::IdConfigured(_) => true, _ => false });
        let video_ok = self.state.lock().unwrap().as_ref().unwrap().video_state.values().all(|val| match *val { MediaState::IdConfigured(_) => true, _ => false });
        if !(audio_ok && video_ok) {
            return Err(gst::error_msg!(gst::LibraryError::Settings, ["Not all pads are fully-configured"]));
        }

        let peer_connection = match self.webrtc_settings.lock().unwrap().config.take() {
            Some(config) => {
                //Acquiring lock before the future instead of cloning because we need to return a value which is dropped with it.
                let mut webrtc_state_lock = self.webrtc_state.lock().unwrap();
                let webrtc_state = webrtc_state_lock.as_mut().unwrap();

                let future = async move {
                    webrtc_state.api.new_peer_connection(config).await.map_err(|e| {
                        gst::error_msg!(
                            gst::ResourceError::Failed,
                            ["Failed to create PeerConnection: {:?}", e]
                        )
                    })
                };

                match self.wait(future) {
                    Ok(peer_connection) => peer_connection,
                    Err(e) => return Err(e.unwrap_or_else(|| gst::error_msg!(gst::ResourceError::Failed, ["Failed to wait for PeerConnection"])))
                }
            }
            None => {
                return Err(gst::error_msg!(gst::LibraryError::Settings, ["WebRTC configuration not set"]));
            }
        };


        self.webrtc_state.lock().unwrap().as_mut().unwrap().peer_connection = Some(peer_connection);


        info!(CAT, "Started");
        Ok(())
    }

    fn stop(&self, _sink: &Self::Type) -> Result<(), ErrorMessage> {
        //Drop states
        self.state.lock().unwrap().take();
        self.webrtc_state.lock().unwrap().take();
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