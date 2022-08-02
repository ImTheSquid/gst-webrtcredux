use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Error};
use gst::{EventView, fixme};
use gst::{error, ErrorMessage, glib, info, prelude::*, traits::{ElementExt, GstObjectExt}};
use gst_video::subclass::prelude::*;
use interceptor::registry::Registry;
use once_cell::sync::Lazy;
use strum_macros::EnumString;
use tokio::runtime;
use webrtc::api::{API, APIBuilder};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_G722, MIME_TYPE_H264, MIME_TYPE_OPUS, MIME_TYPE_PCMA, MIME_TYPE_PCMU, MIME_TYPE_VP8, MIME_TYPE_VP9};
pub use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
pub use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
pub use webrtc::ice_transport::ice_gatherer_state::RTCIceGathererState;
pub use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
pub use webrtc::peer_connection::offer_answer_options::RTCAnswerOptions;
pub use webrtc::peer_connection::offer_answer_options::RTCOfferOptions;
use webrtc::peer_connection::RTCPeerConnection;
pub use webrtc::peer_connection::sdp::sdp_type::RTCSdpType;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use crate::webrtcredux::sender::WebRtcReduxSender;

use super::sdp::SDP;

pub static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        "webrtcredux",
        gst::DebugColorFlags::empty(),
        Some("WebRTC Video and Audio Transmitter"),
    )
});

static RUNTIME: Lazy<runtime::Runtime> = Lazy::new(|| {
    runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
});

#[derive(Debug, PartialEq, Eq, EnumString, Clone, Copy)]
enum MediaType {
    #[strum(
    ascii_case_insensitive,
    serialize = "video/H264",
    serialize = "video/x-h264"
    )]
    H264,
    #[strum(ascii_case_insensitive, serialize = "video/x-vp8")]
    VP8,
    #[strum(ascii_case_insensitive, serialize = "video/x-vp9")]
    VP9,
    #[strum(
    ascii_case_insensitive,
    serialize = "audio/opus",
    serialize = "audio/x-opus"
    )]
    Opus,
    #[strum(ascii_case_insensitive, serialize = "audio/G722")]
    G722,
    #[strum(
    ascii_case_insensitive,
    serialize = "audio/PCMU",
    serialize = "audio/x-mulaw"
    )]
    Mulaw,
    #[strum(
    ascii_case_insensitive,
    serialize = "audio/PCMA",
    serialize = "audio/x-alaw"
    )]
    Alaw,
}

impl MediaType {
    fn webrtc_mime(self) -> &'static str {
        match self {
            MediaType::H264 => MIME_TYPE_H264,
            MediaType::VP8 => MIME_TYPE_VP8,
            MediaType::VP9 => MIME_TYPE_VP9,
            MediaType::Opus => MIME_TYPE_OPUS,
            MediaType::G722 => MIME_TYPE_G722,
            MediaType::Mulaw => MIME_TYPE_PCMU,
            MediaType::Alaw => MIME_TYPE_PCMA,
        }
    }
}

#[derive(Clone)]
enum MediaState {
    NotConfigured,
    IdConfigured(String),
    Configured { track: Arc<TrackLocalStaticSample>, duration: Option<Duration> },
}

#[derive(Debug, Clone)]
struct InputStream {
    sink_pad: gst::GhostPad,
    sender: Option<WebRtcReduxSender>,
}

pub fn make_element(element: &str, name: Option<&str>) -> Result<gst::Element, Error> {
    gst::ElementFactory::make(element, name)
        .with_context(|| format!("Failed to make element {}", element))
}

impl InputStream {
    fn prepare(&mut self, element: &super::WebRtcRedux) -> Result<(), Error> {
        let sender = WebRtcReduxSender::default();

        element.add(&sender).expect("Failed to add sender element");

        self.sender = Some(sender);

        Ok(())
    }

    fn unprepare(&mut self, element: &super::WebRtcRedux) {
        self.sink_pad.set_target(None::<&gst::Pad>).unwrap();

        if let Some(sender) = self.sender.take() {
            element.remove(&sender).unwrap();
            sender.set_state(gst::State::Null).unwrap();
        }
    }
}

impl MediaState {
    fn add_id(&self, new_id: &str) -> Self {
        match self {
            MediaState::NotConfigured => MediaState::IdConfigured(new_id.to_string()),
            MediaState::IdConfigured(_) => MediaState::IdConfigured(new_id.to_string()),
            MediaState::Configured { .. } => self.to_owned(),
        }
    }

    /*fn add_track(&self, track: Arc<TrackLocalStaticSample>, duration: Duration) -> Self {
        match self {
            MediaState::NotConfigured => unreachable!("Shouldn't be able to set track without ID"),
            _ => MediaState::Configured {
                track,
                duration,
            }
        }
    }*/
}

struct WebRtcState {
    api: API,
    peer_connection: Option<RTCPeerConnection>,
}

impl Default for WebRtcState {
    fn default() -> Self {
        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs().expect("Failed to register default codecs");
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine)
            .expect("Failed to register default interceptors");

        WebRtcState {
            api: APIBuilder::new()
                .with_media_engine(media_engine)
                .with_interceptor_registry(registry)
                .build(),
            peer_connection: Default::default(),
        }
    }
}

#[derive(Default)]
struct State {
    video_state: HashMap<usize, String>,
    next_video_pad_id: usize,
    audio_state: HashMap<usize, String>,
    next_audio_pad_id: usize,
    streams: HashMap<String, InputStream>,
}

struct WebRtcSettings {
    config: Option<RTCConfiguration>,
}

impl Default for WebRtcSettings {
    fn default() -> Self {
        WebRtcSettings {
            config: Some(RTCConfiguration::default()),
        }
    }
}

#[derive(Default)]
pub struct WebRtcRedux {
    state: Mutex<State>,
    webrtc_state: Arc<Mutex<WebRtcState>>,
    webrtc_settings: Mutex<WebRtcSettings>,
}

impl WebRtcRedux {
    fn prepare(&self, element: &super::WebRtcRedux) -> Result<(), Error> {
        gst::debug!(CAT, obj: element, "preparing");

        self.state
            .lock()
            .unwrap()
            .streams
            .iter_mut()
            .try_for_each(|(_, stream)| stream.prepare(element))?;

        Ok(())
    }

    fn unprepare(&self, element: &super::WebRtcRedux) -> Result<(), Error> {
        info!(CAT, obj: element, "unpreparing");

        let mut state = self.state.lock().unwrap();

        state
            .streams
            .iter_mut()
            .for_each(|(_, stream)| stream.unprepare(element));
        Ok(())
    }

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

    fn sink_event(&self, pad: &gst::Pad, element: &super::WebRtcRedux, event: gst::Event) -> bool {
        match event.view() {
            EventView::Caps(caps) => {
                self.create_track(&pad.name(), caps);
                pad.event_default(Some(element), event)
            },
            _ => pad.event_default(Some(element), event)
        }
    }

    fn create_track(&self, name: &str, caps: &gst::event::Caps) {
        let name_parts = name.split('_').collect::<Vec<_>>();
        let id: usize = name_parts[1].parse().unwrap();

        let caps = caps.structure().unwrap().get::<gst::Caps>("caps").unwrap();
        let structure = caps.structure(0).unwrap();
        let mime = structure.name();
        let duration = if name.starts_with("video") {
            let framerate = structure.get::<gst::Fraction>("framerate").unwrap().0;
            Some(Duration::from_millis(((*framerate.denom() as f64 / *framerate.numer() as f64)  * 1000.0).round() as u64))
        } else {
            None
        };

        // TODO: Clean up
        let stream_id = if name.starts_with("video") {
            let state = self.state.lock().unwrap();
            let value = state.video_state.get(&id);
            if let Some(value) = value {
                value.to_owned()
            } else {
                fixme!(CAT, "Using pad name as stream_id for video pad {}, consider setting before pipeline starts", name);
                format!("video_{}", id)
            }
        } else {
            let state = self.state.lock().unwrap();
            let value = state.audio_state.get(&id);
            if let Some(value) = value {
                value.to_owned()
            } else {
                fixme!(CAT, "Using pad name as stream_id for video pad {}, consider setting before pipeline starts", name);
                format!("audio_{}", id)
            }
        };

        let track  = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MediaType::from_str(mime).expect("Failed to parse mime type").webrtc_mime().to_string(),
                ..RTCRtpCodecCapability::default()
            }, 
            name_parts[0].to_string(), 
            stream_id
        ));

        let webrtc_state = self.webrtc_state.clone();
        let track_arc = track.clone();
        let rtp_sender = RUNTIME.block_on(async move {
            webrtc_state.lock().unwrap().peer_connection.as_ref().unwrap().add_track(Arc::clone(&track_arc) as Arc<dyn TrackLocal + Send + Sync>).await
        }).expect("Failed to add track");

        thread::spawn(move || {
            RUNTIME.block_on(async move {
                let mut rtcp_buf = vec![0u8; 1500];
                while let Ok((_, _)) = rtp_sender.read(&mut rtcp_buf).await {}
                anyhow::Result::<()>::Ok(())
            })
        });

        self.state.lock().unwrap().streams.get(name).unwrap().sender.as_ref().unwrap().add_info(track, duration);
    }

    pub fn set_stream_id(&self, pad_name: &str, stream_id: &str) -> Result<(), ErrorMessage> {
        let split = pad_name.split('_').collect::<Vec<_>>();
        if split.len() != 2 {
            return Err(gst::error_msg!(
                gst::ResourceError::NotFound,
                [&format!("Pad with name '{}' is invalid", pad_name)]
            ));
        }

        let id: usize = match split[1].parse() {
            Ok(val) => val,
            Err(_) => {
                return Err(gst::error_msg!(
                    gst::ResourceError::NotFound,
                    [&format!("Couldn't parse '{}' into number", split[1])]
                ));
            }
        };

        match split[0] {
            "video" => {
                if !self
                    .state
                    .lock()
                    .unwrap()
                    .video_state
                    .contains_key(&id)
                {
                    return Err(gst::error_msg!(
                        gst::ResourceError::NotFound,
                        [&format!("Invalid ID: {}", id)]
                    ));
                }

                let current = self.state.lock().unwrap().video_state.get(&id)
                    .unwrap()
                    .to_owned();

                self.state.lock().unwrap()
                    .video_state
                    .insert(id, stream_id.to_string());

                Ok(())
            }
            "audio" => {
                if !self
                    .state
                    .lock()
                    .unwrap()
                    .audio_state
                    .contains_key(&id)
                {
                    return Err(gst::error_msg!(
                        gst::ResourceError::NotFound,
                        [&format!("Invalid ID: {}", id)]
                    ));
                }

                let current = self.state.lock().unwrap().video_state.get(&id)
                    .unwrap()
                    .to_owned();

                self.state
                    .lock()
                    .unwrap()
                    .audio_state
                    .insert(id, stream_id.to_string());

                Ok(())
            }
            _ => Err(gst::error_msg!(
                gst::ResourceError::NotFound,
                [&format!("Pad with type '{}' not found", split[0])]
            )),
        }
    }

    pub async fn gathering_complete_promise(&self) -> Result<tokio::sync::mpsc::Receiver<()>, ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        Ok(peer_connection.gathering_complete_promise().await)
    }

    pub async fn create_offer(
        &self,
        options: Option<RTCOfferOptions>,
    ) -> Result<SDP, ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        match peer_connection.create_offer(options).await {
            Ok(res) => Ok(SDP::from_str(&res.sdp).unwrap()),
            Err(e) => Err(gst::error_msg!(
                gst::ResourceError::Failed,
                [&format!("Failed to create offer: {:?}", e)]
            )),
        }
    }

    pub async fn create_answer(
        &self,
        options: Option<RTCAnswerOptions>,
    ) -> Result<SDP, ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        match peer_connection.create_answer(options).await {
            Ok(res) => Ok(SDP::from_str(&res.sdp).unwrap()),
            Err(e) => Err(gst::error_msg!(
                gst::ResourceError::Failed,
                [&format!("Failed to create answer: {:?}", e)]
            )),
        }
    }

    pub async fn local_description(&self) -> Result<Option<SDP>, ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        match peer_connection.local_description().await {
            None => Ok(None),
            Some(res) => Ok(Some(SDP::from_str(&res.sdp).unwrap()))
        }
    }

    pub async fn set_local_description(&self, sdp: &SDP, sdp_type: RTCSdpType) -> Result<(), ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        let mut default = RTCSessionDescription::default();
        default.sdp = sdp.to_string();
        default.sdp_type = sdp_type;

        if let Err(e) = peer_connection.set_local_description(default).await {
            return Err(gst::error_msg!(
                gst::ResourceError::Failed,
                [&format!("Failed to set local description: {:?}", e)]
            ));
        }

        Ok(())
    }

    pub async fn set_remote_description(&self, sdp: &SDP, sdp_type: RTCSdpType) -> Result<(), ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        let mut default = RTCSessionDescription::default();
        default.sdp = sdp.to_string();
        default.sdp_type = sdp_type;

        if let Err(e) = peer_connection.set_remote_description(default).await {
            return Err(gst::error_msg!(
                gst::ResourceError::Failed,
                [&format!("Failed to set local description: {:?}", e)]
            ));
        }

        Ok(())
    }

    pub async fn on_negotiation_needed<F>(&self, mut f: F) -> Result<(), ErrorMessage>
        where
            F: FnMut() + Send + Sync + 'static,
    {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        peer_connection
            .on_negotiation_needed(Box::new(move || {
                f();
                Box::pin(async {})
            }))
            .await;

        Ok(())
    }

    pub async fn on_ice_candidate<F>(&self, mut f: F) -> Result<(), ErrorMessage>
        where
            F: FnMut(Option<RTCIceCandidate>) + Send + Sync + 'static,
    {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        peer_connection
            .on_ice_candidate(Box::new(move |candidate| {
                f(candidate);
                Box::pin(async {})
            }))
            .await;

        Ok(())
    }

    pub async fn on_ice_gathering_state_change<F>(&self, mut f: F) -> Result<(), ErrorMessage>
        where
            F: FnMut(RTCIceGathererState) + Send + Sync + 'static,
    {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        peer_connection
            .on_ice_gathering_state_change(Box::new(move |state| {
                f(state);
                Box::pin(async {})
            }))
            .await;

        Ok(())
    }

    pub async fn on_ice_connection_state_change<F>(&self, mut f: F) -> Result<(), ErrorMessage>
        where
            F: FnMut(RTCIceConnectionState) + Send + Sync + 'static,
    {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        peer_connection
            .on_ice_connection_state_change(Box::new(move |state| {
                f(state);
                Box::pin(async {})
            }))
            .await;

        Ok(())
    }

    pub async fn add_ice_candidate(
        &self,
        candidate: RTCIceCandidateInit,
    ) -> Result<(), ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        if let Err(e) = peer_connection.add_ice_candidate(candidate).await {
            return Err(gst::error_msg!(
                gst::ResourceError::Failed,
                [&format!("Failed to add ICE candidate: {:?}", e)]
            ));
        }

        Ok(())
    }

    fn get_peer_connection(state: &WebRtcState) -> Result<&RTCPeerConnection, ErrorMessage> {
        match &state.peer_connection {
            Some(conn) => Ok(conn),
            None => {
                Err(gst::error_msg!(
                    gst::ResourceError::Failed,
                    ["Peer connection is not set, make sure plugin is started"]
                ))
            }
        }
    }
}

#[glib::object_subclass]
impl ObjectSubclass for WebRtcRedux {
    const NAME: &'static str = "WebRtcRedux";
    type Type = super::WebRtcRedux;
    type ParentType = gst::Bin;
}

impl ElementImpl for WebRtcRedux {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: Lazy<gst::subclass::ElementMetadata> = Lazy::new(|| {
            gst::subclass::ElementMetadata::new(
                "WebRTC Broadcast Engine",
                "Sink/Video/Audio",
                "Broadcasts encoded video and audio",
                "Jack Hogan; Lorenzo Rizzotti <dev@dreaming.codes>",
            )
        });

        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: Lazy<Vec<gst::PadTemplate>> = Lazy::new(|| {
            let caps = gst::Caps::builder_full()
                .structure(gst::Structure::builder("video/x-h264").build())
                .structure(gst::Structure::builder("video/x-vp8").build())
                .structure(gst::Structure::builder("video/x-vp9").build())
                .build();
            let video_pad_template = gst::PadTemplate::new(
                "video_%u",
                gst::PadDirection::Sink,
                gst::PadPresence::Request,
                &caps,
            )
                .unwrap();

            let caps = gst::Caps::builder_full()
                .structure(gst::Structure::builder("audio/x-opus").build())
                .structure(gst::Structure::builder("audio/G722").build())
                .structure(gst::Structure::builder("audio/x-mulaw").build())
                .structure(gst::Structure::builder("audio/x-alaw").build())
                .build();
            let audio_pad_template = gst::PadTemplate::new(
                "audio_%u",
                gst::PadDirection::Sink,
                gst::PadPresence::Request,
                &caps,
            )
                .unwrap();

            vec![video_pad_template, audio_pad_template]
        });

        PAD_TEMPLATES.as_ref()
    }

    fn request_new_pad(
        &self,
        element: &Self::Type,
        templ: &gst::PadTemplate,
        _name: Option<String>,
        _caps: Option<&gst::Caps>,
    ) -> Option<gst::Pad> {
        //
        if element.current_state() > gst::State::Ready {
            error!(CAT, "element pads can only be requested before starting");
            return None;
        }

        let mut state = self.state.lock().unwrap();

        let name = if templ.name().starts_with("video_") {
            let name = format!("video_{}", state.next_video_pad_id);
            state.next_video_pad_id += 1;
            name
        } else {
            let name = format!("audio_{}", state.next_audio_pad_id);
            state.next_audio_pad_id += 1;
            name
        };

        let sink_pad = gst::GhostPad::builder_with_template(templ, Some(name.as_str()))
            .event_function(|pad, parent, event| {
                WebRtcRedux::catch_panic_pad_function(
                    parent,
                    || false,
                    |sink, element| sink.sink_event(pad.upcast_ref(), element, event),
                )
            })
            .build();

        sink_pad.set_active(true).unwrap();
        sink_pad.use_fixed_caps();
        element.add_pad(&sink_pad).unwrap();

        state.streams.insert(
            name,
            InputStream {
                sink_pad: sink_pad.clone(),
                sender: None,
            },
        );

        Some(sink_pad.upcast())
    }

    fn change_state(
        &self,
        element: &Self::Type,
        transition: gst::StateChange,
    ) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
        if let gst::StateChange::ReadyToPaused = transition {
            if let Err(err) = self.prepare(element) {
                gst::element_error!(
                    element,
                    gst::StreamError::Failed,
                    ["Failed to prepare: {}", err]
                );
                return Err(gst::StateChangeError);
            }
        }

        let mut ret = self.parent_change_state(element, transition);

        match transition {
            gst::StateChange::NullToReady => {
                let peer_connection = match self.webrtc_settings.lock().unwrap().config.take() {
                    Some(config) => {
                        //Acquiring lock before the future instead of cloning because we need to return a value which is dropped with it.
                        let webrtc_state = self.webrtc_state.lock().unwrap();

                        let future = async move {
                            //TODO: Fix mutex with an async safe mutex
                            webrtc_state
                                .api
                                .new_peer_connection(config)
                                .await
                                .map_err(|e| {
                                    gst::error_msg!(
                                gst::ResourceError::Failed,
                                ["Failed to create PeerConnection: {:?}", e]
                            )
                                })
                        };

                        RUNTIME.block_on(future).unwrap()
                    }
                    None => {
                        return Err(gst::StateChangeError);
                    }
                };

                let _ = self.webrtc_state
                    .lock()
                    .unwrap()
                    .peer_connection.insert(peer_connection);
            }
            gst::StateChange::PausedToReady => {
                if let Err(err) = self.unprepare(element) {
                    gst::element_error!(
                        element,
                        gst::StreamError::Failed,
                        ["Failed to unprepare: {}", err]
                    );
                    return Err(gst::StateChangeError);
                }
            }
            gst::StateChange::ReadyToPaused => {
                ret = Ok(gst::StateChangeSuccess::NoPreroll);
            }
            _ => (),
        }

        ret
    }
}

//TODO: Add signals
impl ObjectImpl for WebRtcRedux {}

impl GstObjectImpl for WebRtcRedux {}

impl BinImpl for WebRtcRedux {}