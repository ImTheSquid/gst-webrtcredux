use std::collections::HashMap;
use std::future::Future;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::future;
use gst::{
    gst_debug as debug,
    gst_error as error,
    gst_info as info,
    gst_trace as trace,
    gst_fixme as fixme,
    glib,
    prelude::PadExtManual,
    traits::{ElementExt, GstObjectExt},
    ErrorMessage,
};
use gst_base::subclass::prelude::*;
use interceptor::registry::Registry;
use once_cell::sync::Lazy;
use strum_macros::EnumString;
use tokio::runtime;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264, MIME_TYPE_OPUS, MIME_TYPE_G722, MIME_TYPE_VP8, MIME_TYPE_VP9, MIME_TYPE_PCMU, MIME_TYPE_PCMA};
use webrtc::api::{APIBuilder, API};
pub use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
pub use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
pub use webrtc::ice_transport::ice_gatherer_state::RTCIceGathererState;
pub use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
pub use webrtc::peer_connection::offer_answer_options::RTCAnswerOptions;
pub use webrtc::peer_connection::offer_answer_options::RTCOfferOptions;
pub use webrtc::peer_connection::sdp::sdp_type::RTCSdpType;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc_media::Sample;

use super::sdp::SDP;

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
    Configured { track: Arc<TrackLocalStaticSample>, duration: Duration },
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

#[derive(Default)]
struct State {
    video_state: HashMap<usize, MediaState>,
    next_video_pad_id: usize,
    audio_state: HashMap<usize, MediaState>,
    next_audio_pad_id: usize,
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

struct GenericSettings {
    timeout: u16,
}

impl Default for GenericSettings {
    fn default() -> Self {
        Self { timeout: 15 }
    }
}

#[derive(Default)]
pub struct WebRtcRedux {
    state: Arc<Mutex<Option<State>>>,
    webrtc_state: Arc<Mutex<Option<WebRtcState>>>,
    settings: Arc<Mutex<GenericSettings>>,
    webrtc_settings: Arc<Mutex<WebRtcSettings>>,
    canceller: Mutex<Option<future::AbortHandle>>,
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
        F: Send + Future<Output = Result<T, ErrorMessage>>,
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
                let res =
                    tokio::time::timeout(std::time::Duration::from_secs(timeout.into()), future)
                        .await;

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
                ))
            }
        };

        match split[0] {
            "video" => {
                if !self
                    .state
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .video_state
                    .contains_key(&id)
                {
                    return Err(gst::error_msg!(
                        gst::ResourceError::NotFound,
                        [&format!("Invalid ID: {}", id)]
                    ));
                }

                let current = {
                    let state = self.state.lock().unwrap();
                    state
                        .as_ref()
                        .unwrap()
                        .video_state
                        .get(&id)
                        .unwrap()
                        .to_owned()
                };

                self.state
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .video_state
                    .insert(id, current.add_id(stream_id));

                Ok(())
            }
            "audio" => {
                if !self
                    .state
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .audio_state
                    .contains_key(&id)
                {
                    return Err(gst::error_msg!(
                        gst::ResourceError::NotFound,
                        [&format!("Invalid ID: {}", id)]
                    ));
                }

                let current = {
                    let state = self.state.lock().unwrap();
                    state
                        .as_ref()
                        .unwrap()
                        .audio_state
                        .get(&id)
                        .unwrap()
                        .to_owned()
                };

                self.state
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .audio_state
                    .insert(id, current.add_id(stream_id));

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
        let peer_connection = WebRtcRedux::get_peer_connection(webrtc_state.as_ref().unwrap())?;

        Ok(peer_connection.gathering_complete_promise().await)
    }

    pub async fn create_offer(
        &self,
        options: Option<RTCOfferOptions>,
    ) -> Result<SDP, ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(webrtc_state.as_ref().unwrap())?;

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
        options: Option<RTCAnswerOptions>
    ) -> Result<SDP, ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(webrtc_state.as_ref().unwrap())?;

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
        let peer_connection = WebRtcRedux::get_peer_connection(webrtc_state.as_ref().unwrap())?;

        match peer_connection.local_description().await {
            None => Ok(None),
            Some(res) => Ok(Some(SDP::from_str(&res.sdp).unwrap()))
        }
    }

    pub async fn set_local_description(&self, sdp: &SDP, sdp_type: RTCSdpType) -> Result<(), ErrorMessage> {
        let webrtc_state = self.webrtc_state.lock().unwrap();
        let peer_connection = WebRtcRedux::get_peer_connection(webrtc_state.as_ref().unwrap())?;

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
        let peer_connection = WebRtcRedux::get_peer_connection(webrtc_state.as_ref().unwrap())?;

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
        let peer_connection = WebRtcRedux::get_peer_connection(webrtc_state.as_ref().unwrap())?;

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
        let peer_connection = WebRtcRedux::get_peer_connection(webrtc_state.as_ref().unwrap())?;

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
        let peer_connection = WebRtcRedux::get_peer_connection(webrtc_state.as_ref().unwrap())?;

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
        let peer_connection = WebRtcRedux::get_peer_connection(webrtc_state.as_ref().unwrap())?;

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
        let peer_connection = WebRtcRedux::get_peer_connection(webrtc_state.as_ref().unwrap())?;

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
    type ParentType = gst_base::BaseSink;

    fn with_class(_klass: &Self::Class) -> Self {
        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs().expect("Failed to register default codecs");
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine)
            .expect("Failed to register default interceptors");
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

        Self {
            state,
            webrtc_state,
            settings,
            webrtc_settings,
            canceller: Mutex::new(None),
        }
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
            let mut video_caps = gst::Caps::new_simple("video/x-h264", &[]);
            let video_caps = video_caps.get_mut().unwrap();
            //MIME_TYPE_VP8
            video_caps.append(gst::Caps::new_simple("video/x-vp8", &[]));
            //MIME_TYPE_VP9
            video_caps.append(gst::Caps::new_simple("video/x-vp9", &[]));

            let video_sink = gst::PadTemplate::new(
                "video_%u",
                gst::PadDirection::Sink,
                gst::PadPresence::Request,
                &video_caps.to_owned(),
            )
            .unwrap();

            //MIME_TYPE_OPUS
            let mut audio_caps = gst::Caps::new_simple("audio/x-opus", &[]);
            let audio_caps = audio_caps.get_mut().unwrap();
            //MIME_TYPE_G722
            audio_caps.append(gst::Caps::new_simple("audio/G722", &[]));
            //MIME_TYPE_PCMU
            audio_caps.append(gst::Caps::new_simple("audio/x-mulaw", &[]));
            //MIME_TYPE_PCMA
            audio_caps.append(gst::Caps::new_simple("audio/x-alaw", &[]));

            let audio_sink = gst::PadTemplate::new(
                "audio_%u",
                gst::PadDirection::Sink,
                gst::PadPresence::Request,
                &audio_caps.to_owned(),
            )
            .unwrap();

            vec![video_sink, audio_sink]
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
        let mut state = self.state.lock().unwrap();
        let state = state.as_mut().unwrap();

        // Set up audio and video pads along with callbacks for events and data
        match templ.name_template().unwrap().as_str() {
            "video_%u" => {
                let pad = gst::Pad::from_template(
                    templ,
                    Some(&format!("video_{}", state.next_video_pad_id)),
                );
                unsafe {
                    let id = state.next_video_pad_id;
                    let state = self.state.clone();
                    let webrtc_state = self.webrtc_state.clone();
                    pad.set_event_function(move |_pad, _parent, event| {
                        let structure = event.structure().unwrap();
                        if structure.name() == "GstEventCaps" {
                            let structure = structure
                                .get::<gst::Caps>("caps")
                                .unwrap();

                            let structure = structure.structure(0).unwrap();

                            let mime = structure.name();

                            let framerate = structure.get::<gst::Fraction>("framerate").unwrap().0;
                            let duration = Duration::from_millis(((*framerate.denom() as f64 / *framerate.numer() as f64)  * 1000.0).round() as u64);

                            let stream_id = match {
                                let state = state.lock().unwrap();
                                state
                                    .as_ref()
                                    .unwrap()
                                    .video_state
                                    .get(&id)
                                    .unwrap()
                                    .to_owned()
                            } {
                                MediaState::IdConfigured(id) => id,
                                _ => unreachable!()
                            };

                            let track = Arc::new(TrackLocalStaticSample::new(
                                RTCRtpCodecCapability { 
                                    mime_type: MediaType::from_str(mime).expect("Failed to parse mime type").webrtc_mime().to_string(),
                                    ..Default::default()
                                }, 
                                "video".to_string(), 
                                stream_id
                            ));

                            // Add track to connection
                            let webrtc_state = webrtc_state.clone();
                            let video = track.clone();
                            let rtp_sender = RUNTIME.block_on(async move {
                                webrtc_state.lock().unwrap().as_ref().unwrap().peer_connection.as_ref().unwrap().add_track(Arc::clone(&video) as Arc<dyn TrackLocal + Send + Sync>).await
                            }).expect("Failed to add track");

                            thread::spawn(move || {
                                RUNTIME.block_on(async move {
                                    let mut rtcp_buf = vec![0u8; 1500];
                                    while let Ok((_, _)) = rtp_sender.read(&mut rtcp_buf).await {}
                                    anyhow::Result::<()>::Ok(())
                                })
                            });
                            
                            state.lock().unwrap().as_mut().unwrap().video_state.insert(
                                id,
                                MediaState::Configured { track, duration },
                            );

                            debug!(CAT, "Video media type set to: {}", mime);
                        }
                        true
                    });

                    let chain_state = self.state.clone();
                    let last_sample_timestamp: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
                    pad.set_chain_function(move |_pad, _parent, buffer| {
                        let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                        trace!(CAT, "Video map size: {}", map.size());
                        let chain_state = chain_state.lock().unwrap();
                        let (track, duration) = match chain_state.as_ref().unwrap().video_state.get(&id).unwrap() {
                            MediaState::Configured { track, duration} => (track, duration.to_owned()),
                            _ => return Ok(gst::FlowSuccess::Ok)
                        };

                        let diff_duration = if let Some(last) = last_sample_timestamp.lock().unwrap().as_ref() {
                            Instant::now() - *last
                        } else {
                            Duration::from_millis(0)
                        };

                        RUNTIME.block_on(async move {
                            track.write_sample(&Sample {
                                data: Bytes::copy_from_slice(map.as_slice()),
                                duration: Duration::from_millis((duration.as_millis() * diff_duration.as_millis()).try_into().unwrap()),
                                ..Sample::default()
                            }).await
                        }).expect("Failed to write sample");

                        let _ = last_sample_timestamp.lock().unwrap().insert(Instant::now());

                        Ok(gst::FlowSuccess::Ok)
                    });
                }
                element.add_pad(&pad).unwrap();

                state
                    .video_state
                    .insert(state.next_video_pad_id, MediaState::NotConfigured);
                state.next_video_pad_id += 1;

                Some(pad)
            }
            "audio_%u" => {
                let pad = gst::Pad::from_template(
                    templ,
                    Some(&format!("audio_{}", state.next_audio_pad_id)),
                );
                unsafe {
                    let id = state.next_audio_pad_id;
                    let state = self.state.clone();
                    let webrtc_state = self.webrtc_state.clone();
                    pad.set_event_function(move |_pad, _parent, event| {
                        let structure = event.structure().unwrap();
                        if structure.name() == "GstEventCaps" {
                            let structure = structure
                                .get::<gst::Caps>("caps")
                                .unwrap();

                            let structure = structure.structure(0).unwrap();

                            let mime = structure.name();

                            let sample_rate: i32 = structure.get("rate").unwrap();
                            let duration = Duration::from_millis(((1.0 / sample_rate as f64)  * 1000.0).round() as u64);

                            let stream_id = match {
                                let state = state.lock().unwrap();
                                state
                                    .as_ref()
                                    .unwrap()
                                    .audio_state
                                    .get(&id)
                                    .unwrap()
                                    .to_owned()
                            } {
                                MediaState::IdConfigured(id) => id,
                                _ => unreachable!()
                            };

                            let track = Arc::new(TrackLocalStaticSample::new(
                                RTCRtpCodecCapability { 
                                    mime_type: MediaType::from_str(mime).expect("Failed to parse mime type").webrtc_mime().to_string(), 
                                    ..RTCRtpCodecCapability::default()
                                }, 
                                "audio".to_string(), 
                                stream_id
                            ));
                            
                            // Add track to connection
                            let webrtc_state = webrtc_state.clone();
                            let audio = track.clone();
                            let rtp_sender = RUNTIME.block_on(async move {
                                webrtc_state.lock().unwrap().as_ref().unwrap().peer_connection.as_ref().unwrap().add_track(Arc::clone(&audio) as Arc<dyn TrackLocal + Send + Sync>).await
                            }).expect("Failed to add track");

                            thread::spawn(move || {
                                RUNTIME.block_on(async move {
                                    let mut rtcp_buf = vec![0u8; 1500];
                                    while let Ok((_, _)) = rtp_sender.read(&mut rtcp_buf).await {}
                                    anyhow::Result::<()>::Ok(())
                                })
                            });
                            
                            state.lock().unwrap().as_mut().unwrap().audio_state.insert(
                                id,
                                MediaState::Configured { track, duration },
                            );

                            debug!(CAT, "Audio media type set to: {}", mime);
                        }
                        true
                    });

                    let chain_state = self.state.clone();
                    let last_sample_timestamp: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
                    pad.set_chain_function(move |_pad, _parent, buffer| {
                        let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                        trace!(CAT, "Audio map size: {}", map.size());
                        let chain_state = chain_state.lock().unwrap();
                        let (track, duration) = match chain_state.as_ref().unwrap().audio_state.get(&id).unwrap() {
                            MediaState::Configured { track, duration} => (track, duration.to_owned()),
                            _ => return Ok(gst::FlowSuccess::Ok)
                        };

                        let diff_duration = if let Some(last) = last_sample_timestamp.lock().unwrap().as_ref() {
                            Instant::now() - *last
                        } else {
                            Duration::from_millis(0)
                        };

                        RUNTIME.block_on(async move {
                            track.write_sample(&Sample {
                                data: Bytes::copy_from_slice(map.as_slice()),
                                duration: Duration::from_millis((duration.as_millis() * diff_duration.as_millis()).try_into().unwrap()),
                                ..Sample::default()
                            }).await
                        }).expect("Failed to write sample");

                        let _ = last_sample_timestamp.lock().unwrap().insert(Instant::now());

                        Ok(gst::FlowSuccess::Ok)
                    });
                }
                element.add_pad(&pad).unwrap();

                state
                    .audio_state
                    .insert(state.next_audio_pad_id, MediaState::NotConfigured);
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
        if let None = self.state.lock().unwrap().as_ref() {
            return;
        }

        let name = pad.name();
        let split = name.split('_').collect::<Vec<_>>();
        let id: usize = split[1].parse().unwrap();
        if split[0] == "video" {
            self.state
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .video_state
                .remove(&id);
        } else {
            self.state
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .audio_state
                .remove(&id);
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
        for (key, val) in self.state.lock().unwrap().as_mut().unwrap().audio_state.iter_mut().filter(|(_, val)| match **val { MediaState::NotConfigured => true, _ => false }) {
            fixme!(CAT, "Using pad name as stream_id for audio pad {}, consider setting before pipeline starts", key);
            *val = val.add_id(&format!("audio_{}", key));
        }

        for (key, val) in self.state.lock().unwrap().as_mut().unwrap().video_state.iter_mut().filter(|(_, val)| match **val { MediaState::NotConfigured => true, _ => false }) {
            fixme!(CAT, "Using pad name as stream_id for video pad {}, consider setting before pipeline starts", key);
            *val = val.add_id(&format!("video_{}", key));
        }

        let peer_connection = match self.webrtc_settings.lock().unwrap().config.take() {
            Some(config) => {
                //Acquiring lock before the future instead of cloning because we need to return a value which is dropped with it.
                let mut webrtc_state_lock = self.webrtc_state.lock().unwrap();
                let webrtc_state = webrtc_state_lock.as_mut().unwrap();

                let future = async move {
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

                match self.wait(future) {
                    Ok(peer_connection) => peer_connection,
                    Err(e) => {
                        return Err(e.unwrap_or_else(|| {
                            gst::error_msg!(
                                gst::ResourceError::Failed,
                                ["Failed to wait for PeerConnection"]
                            )
                        }))
                    }
                }
            }
            None => {
                return Err(gst::error_msg!(
                    gst::LibraryError::Settings,
                    ["WebRTC configuration not set"]
                ));
            }
        };

        self.webrtc_state
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .peer_connection = Some(peer_connection);

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
