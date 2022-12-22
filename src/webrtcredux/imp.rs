use std::collections::HashMap;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use futures::Future;
use futures::executor::block_on;
use tokio::sync::{Mutex as AsyncMutex, oneshot};

use anyhow::{Context, Error};
use gst::{debug, error, info, fixme, ErrorMessage, glib, prelude::*, traits::{ElementExt, GstObjectExt}, EventView};
use gst_video::subclass::prelude::*;
use interceptor::registry::Registry;
use once_cell::sync::Lazy;
use strum_macros::EnumString;
use tokio::runtime::{self, Handle};
use webrtc::api::{API, APIBuilder};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_G722, MIME_TYPE_H264, MIME_TYPE_OPUS, MIME_TYPE_PCMA, MIME_TYPE_PCMU, MIME_TYPE_VP8, MIME_TYPE_VP9};
pub use webrtc::data_channel::RTCDataChannel;
pub use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
pub use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
pub use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_gatherer::{OnLocalCandidateHdlrFn, OnICEGathererStateChangeHdlrFn};
pub use webrtc::ice_transport::ice_gatherer_state::RTCIceGathererState;
pub use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
pub use webrtc::peer_connection::offer_answer_options::RTCAnswerOptions;
pub use webrtc::peer_connection::offer_answer_options::RTCOfferOptions;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::{RTCPeerConnection, OnNegotiationNeededHdlrFn, OnICEConnectionStateChangeHdlrFn, OnPeerConnectionStateChangeHdlrFn};
pub use webrtc::peer_connection::policy::bundle_policy::RTCBundlePolicy;
pub use webrtc::peer_connection::policy::sdp_semantics::RTCSdpSemantics;
pub use webrtc::peer_connection::sdp::sdp_type::RTCSdpType;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
pub use webrtc::rtp_transceiver::{RTCRtpTransceiverInit, RTCRtpTransceiver};
pub use webrtc::rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTPCodecType};
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use crate::sdp::LineEnding;
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

pub type OnAllTracksAddedFn = Box<dyn FnMut() -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> + Send + Sync>;

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

#[derive(Debug, Clone)]
struct InputStream {
    sink_pad: gst::GhostPad,
    sender: Option<WebRtcReduxSender>,
}

pub fn make_element(element: &str) -> Result<gst::Element, Error> {
    gst::ElementFactory::make(element)
        .build()
        .with_context(|| format!("Failed to make element {}", element))
}

impl InputStream {
    fn prepare(&mut self, element: &super::WebRtcRedux) -> Result<(), Error> {
        let sender = WebRtcReduxSender::default();

        element.add(&sender).expect("Failed to add sender element");

        element
            .sync_children_states()
            .with_context(|| format!("Linking input stream {}", self.sink_pad.name()))?;

        self.sink_pad
            .set_target(Some(&sender.static_pad("sink").unwrap()))
            .unwrap();

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

struct WebRtcState {
    api: API,
    peer_connection: Option<RTCPeerConnection>
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
            peer_connection: Default::default()
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
    handle: Option<Handle>,
    on_all_tracks_added_send: Option<oneshot::Sender<()>>,
    on_all_tracks_added: Option<oneshot::Receiver<()>>,
    on_peer_connection_send: Arc<Mutex<Option<Vec<oneshot::Sender<()>>>>>,
    on_peer_connection_fn: Arc<Mutex<Option<OnPeerConnectionStateChangeHdlrFn>>>,
    tracks: usize
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
    webrtc_state: Arc<AsyncMutex<WebRtcState>>,
    webrtc_settings: Mutex<WebRtcSettings>,
}

impl WebRtcRedux {
    fn prepare(&self, element: &super::WebRtcRedux) -> Result<(), Error> {
        debug!(CAT, obj: element, "preparing");

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

    pub fn set_bundle_policy(&self, bundle_policy: RTCBundlePolicy) {
        let mut webrtc_settings = self.webrtc_settings.lock().unwrap();

        match webrtc_settings.config {
            Some(ref mut config) => {
                config.bundle_policy = bundle_policy;
            }
            None => {
                error!(CAT, "Trying to set bundle policy after starting");
            }
        }
    }

    fn sink_event(&self, pad: &gst::Pad, element: &super::WebRtcRedux, event: gst::Event) -> bool {
        if let EventView::Caps(caps) = event.view() {
            self.create_track(&pad.name(), caps);
        }
        gst::Pad::event_default(pad, Some(element), event)
    }

    fn create_track(&self, name: &str, caps: &gst::event::Caps) {
        let name_parts = name.split('_').collect::<Vec<_>>();
        let id: usize = name_parts[1].parse().unwrap();

        let caps = caps.structure().unwrap().get::<gst::Caps>("caps").unwrap();
        let structure = caps.structure(0).unwrap();
        let mime = structure.name();
        let duration = if name.starts_with("video") {
            let framerate = structure.get::<gst::Fraction>("framerate").unwrap().0;
            Some(gst::ClockTime::from_mseconds(((*framerate.denom() as f64 / *framerate.numer() as f64)  * 1000.0).round() as u64))
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
        let handle = self.runtime_handle();
        let inner = handle.clone();
        let rtp_sender = block_on(async move {
            handle.spawn_blocking(move || {
                inner.block_on(async move {
                    webrtc_state.lock().await.peer_connection.as_ref().unwrap().add_track(Arc::clone(&track_arc) as Arc<dyn TrackLocal + Send + Sync>).await
                })
            }).await
        }).unwrap().unwrap();

        self.runtime_handle().spawn(async move {
            let mut rtcp_buf = vec![0u8; 1500];
            while let Ok((_, _)) = rtp_sender.read(&mut rtcp_buf).await {}
            anyhow::Result::<()>::Ok(())
        });

        let media_type = match name_parts[0] {
            "video" => crate::webrtcredux::sender::MediaType::Video,
            "audio" => crate::webrtcredux::sender::MediaType::Audio,
            _ => unreachable!()
        };

        // Moving this out of the add_info call fixed a lockup, I'm not gonna question why
        let handle = self.runtime_handle();
        let (tx, rx) = oneshot::channel::<()>();
        self.state.lock().unwrap().on_peer_connection_send.lock().unwrap().get_or_insert(vec![]).push(tx);
        self.state.lock().unwrap().streams.get(name).unwrap().sender.as_ref().unwrap().add_info(track, handle, media_type, duration, rx);

        self.state.lock().unwrap().tracks += 1;
        {
            let mut state = self.state.lock().unwrap();
            if state.tracks == state.next_audio_pad_id + state.next_video_pad_id {
                debug!(CAT, "All {} tracks added", state.tracks);
                state.on_all_tracks_added_send.take().unwrap().send(()).unwrap();
            }
        }
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

    pub async fn add_transceiver_from_kind(
        &self,
        codec_type: RTPCodecType,
        init_params: &[RTCRtpTransceiverInit]
    ) -> Result<Arc<RTCRtpTransceiver>, ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().await;
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        match peer_connection.add_transceiver_from_kind(codec_type, init_params).await
        {
            Ok(res) => Ok(res),
            Err(e) => Err(gst::error_msg!(
                gst::ResourceError::Failed,
                [&format!("Failed to create transceiver: {:?}", e)]
            )),
        }
    }

    pub async fn gathering_complete_promise(&self) -> Result<tokio::sync::mpsc::Receiver<()>, ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().await;
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        Ok(peer_connection.gathering_complete_promise().await)
    }

    pub async fn create_offer(
        &self,
        options: Option<RTCOfferOptions>,
    ) -> Result<SDP, ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().await;
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
        let webrtc_state = self.webrtc_state.lock().await;
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
        let webrtc_state = self.webrtc_state.lock().await;
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        match peer_connection.local_description().await {
            None => Ok(None),
            Some(res) => Ok(Some(SDP::from_str(&res.sdp).unwrap()))
        }
    }

    pub async fn set_local_description(&self, sdp: &SDP, sdp_type: RTCSdpType) -> Result<(), ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().await;
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        let mut default = RTCSessionDescription::default();
        default.sdp = sdp.to_string(LineEnding::CRLF);
        default.sdp_type = sdp_type;

        if let Err(e) = peer_connection.set_local_description(default).await {
            return Err(gst::error_msg!(
                gst::ResourceError::Failed,
                [&format!("Failed to set local description: {:?}", e)]
            ));
        }

        Ok(())
    }

    pub async fn remote_description(&self) -> Result<Option<SDP>, ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().await;
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        match peer_connection.remote_description().await {
            None => Ok(None),
            Some(res) => Ok(Some(SDP::from_str(&res.sdp).unwrap()))
        }
    }

    pub async fn set_remote_description(&self, sdp: &SDP, sdp_type: RTCSdpType) -> Result<(), ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().await;
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        let mut default = RTCSessionDescription::default();
        default.sdp = sdp.to_string(LineEnding::CRLF);
        default.sdp_type = sdp_type;

        if let Err(e) = peer_connection.set_remote_description(default).await {
            return Err(gst::error_msg!(
                gst::ResourceError::Failed,
                [&format!("Failed to set remote description: {:?}", e)]
            ));
        }

        Ok(())
    }

    pub async fn on_negotiation_needed(&self, f: OnNegotiationNeededHdlrFn) -> Result<(), ErrorMessage>
    {
        let webrtc_state = self.webrtc_state.lock().await;
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        peer_connection
            .on_negotiation_needed(Box::new(f));

        Ok(())
    }

    pub async fn on_ice_candidate(&self, f: OnLocalCandidateHdlrFn) -> Result<(), ErrorMessage>
    {
        let webrtc_state = self.webrtc_state.lock().await;
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        peer_connection
            .on_ice_candidate(Box::new(f));

        Ok(())
    }

    pub async fn on_ice_gathering_state_change(&self, f: OnICEGathererStateChangeHdlrFn) -> Result<(), ErrorMessage>
    {
        let webrtc_state = self.webrtc_state.lock().await;
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        peer_connection
            .on_ice_gathering_state_change(Box::new(f));

        Ok(())
    }

    pub async fn on_ice_connection_state_change(&self, f: OnICEConnectionStateChangeHdlrFn) -> Result<(), ErrorMessage>
    {
        let webrtc_state = self.webrtc_state.lock().await;
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        peer_connection
            .on_ice_connection_state_change(Box::new(f));

        Ok(())
    }

    pub fn on_peer_connection_state_change(&self, f: OnPeerConnectionStateChangeHdlrFn) -> Result<(), ErrorMessage> {
        // peer_connection
        //     .on_peer_connection_state_change(Box::new(f));
        let _ = self.state.lock().unwrap().on_peer_connection_fn.lock().unwrap().insert(f);

        Ok(())
    }

    pub async fn add_ice_candidate(
        &self,
        candidate: RTCIceCandidateInit,
    ) -> Result<(), ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().await;
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        if let Err(e) = peer_connection.add_ice_candidate(candidate).await {
            return Err(gst::error_msg!(
                gst::ResourceError::Failed,
                [&format!("Failed to add ICE candidate: {:?}", e)]
            ));
        }

        Ok(())
    }

    pub async fn create_data_channel(&self,
        name: &str,
        init_params: Option<RTCDataChannelInit>
    ) -> Result<Arc<RTCDataChannel>, ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().await;
        let peer_connection = WebRtcRedux::get_peer_connection(&webrtc_state)?;

        match peer_connection.create_data_channel(name, init_params).await {
            Ok(res) => Ok(res),
            Err(e) => {
                Err(gst::error_msg!(
                    gst::ResourceError::Failed,
                    [&format!("Failed to create data channel: {:?}", e)]
                ))
            }
        }
    }

    pub fn set_tokio_runtime(
        &self,
        handle: Handle
    ) {
        let _ = self.state.lock().unwrap().handle.insert(handle);
    }

    pub async fn wait_for_all_tracks(&self) {
        let all = self.state.lock().unwrap().on_all_tracks_added.take().unwrap();
        all.await.unwrap();
    }

    fn runtime_handle(&self) -> Handle {
        self.state.lock().unwrap().handle.as_ref().unwrap_or(RUNTIME.handle()).clone()
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
                .structure(gst::Structure::builder("video/x-h264").field("stream-format", "byte-stream").field("profile", "baseline").build())
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
        templ: &gst::PadTemplate,
        _name: Option<&str>,
        _caps: Option<&gst::Caps>,
    ) -> Option<gst::Pad> {
        let element = self.obj();
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
                    |this| this.sink_event(pad.upcast_ref(), &this.obj(), event),
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
        transition: gst::StateChange,
    ) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
        let element = self.obj();
        if let gst::StateChange::ReadyToPaused = transition {
            if let Err(err) = self.prepare(&element) {
                gst::element_error!(
                    element,
                    gst::StreamError::Failed,
                    ["Failed to prepare: {}", err]
                );
                return Err(gst::StateChangeError);
            }
        }

        let mut ret = self.parent_change_state(transition);

        match transition {
            gst::StateChange::NullToReady => {
                match self.webrtc_settings.lock().unwrap().config.take() {
                    Some(config) => {
                        //Acquiring lock before the future instead of cloning because we need to return a value which is dropped with it.
                        let webrtc_state = self.webrtc_state.clone();
                        let on_pc_send = self.state.lock().unwrap().on_peer_connection_send.clone();
                        let on_pc_fn = self.state.lock().unwrap().on_peer_connection_fn.clone();

                        {
                            let (tx, rx) = oneshot::channel();
                            let mut state = self.state.lock().unwrap();
                            let _ = state.on_all_tracks_added_send.insert(tx);
                            let _ = state.on_all_tracks_added.insert(rx);
                        }

                        let handle = self.runtime_handle();
                        let inner = handle.clone();
                        
                        block_on(async move {
                            handle.spawn_blocking(move || {
                                inner.block_on(async move {
                                    let mut webrtc_state = webrtc_state.lock().await;
                                    //TODO: Fix mutex with an async safe mutex
                                    let peer_connection = webrtc_state
                                        .api
                                        .new_peer_connection(config)
                                        .await
                                        .map_err(|e| {
                                            gst::error_msg!(
                                                gst::ResourceError::Failed,
                                                ["Failed to create PeerConnection: {:?}", e]
                                            )
                                        });
        
                                    match peer_connection {
                                        Ok(conn) => {
                                            conn.on_peer_connection_state_change(Box::new(move |state| {
                                                // Notify sender elements when peer is connected
                                                if state == RTCPeerConnectionState::Connected {
                                                    if let Some(vec) = on_pc_send.lock().unwrap().take() {
                                                        for send in vec.into_iter() {
                                                            send.send(()).unwrap();
                                                        }
                                                    }
                                                }

                                                // Run user-defined callback function if it exists
                                                let mut on_pc_fn = on_pc_fn.lock().unwrap();
                                                if on_pc_fn.is_some() {on_pc_fn.as_mut().unwrap()(state)} else {Box::pin(async {})}
                                            }));

                                            let _ = webrtc_state.peer_connection.insert(conn);

                                            Ok(())
                                        },
                                        Err(e) => Err(e)
                                    }
                                }).unwrap();
                            }).await
                        }).unwrap();
                    }
                    None => {
                        return Err(gst::StateChangeError);
                    }
                }
            }
            gst::StateChange::PausedToReady => {
                if let Err(err) = self.unprepare(&element) {
                    gst::element_error!(
                        element,
                        gst::StreamError::Failed,
                        ["Failed to unprepare: {}", err]
                    );
                    return Err(gst::StateChangeError);
                }
            }
            gst::StateChange::ReadyToNull => {
                //Acquiring lock before the future instead of cloning because we need to return a value which is dropped with it.
                let webrtc_state = self.webrtc_state.clone();

                let handle = self.runtime_handle();
                let inner = handle.clone();

                block_on(async move {
                    handle.spawn_blocking(move || {
                        inner.block_on(async move {
                            let mut webrtc_state = webrtc_state.lock().await;
                            //TODO: Fix mutex with an async safe mutex
                            if let Some(conn) = webrtc_state.peer_connection.take() {
                                conn.close().await
                            } else {
                                Ok(())
                            }
                        })
                    }).await
                }).unwrap().unwrap();
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