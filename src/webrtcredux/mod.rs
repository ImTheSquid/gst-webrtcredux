use std::sync::Arc;

use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::ObjectSubclassExt;
use gst::ErrorMessage;

mod sender;

mod imp;

pub use imp::*;
use tokio::runtime::Handle;
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::ice_transport::ice_gatherer::OnICEGathererStateChangeHdlrFn;
use webrtc::ice_transport::ice_gatherer::OnLocalCandidateHdlrFn;
use webrtc::peer_connection::OnICEConnectionStateChangeHdlrFn;
use webrtc::peer_connection::OnNegotiationNeededHdlrFn;
use webrtc::peer_connection::OnPeerConnectionStateChangeHdlrFn;
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;

use self::sdp::SDP;
pub mod sdp;

glib::wrapper! {
    pub struct WebRtcRedux(ObjectSubclass<imp::WebRtcRedux>) @extends gst::Bin, gst::Element, gst::Object;
}

impl Default for WebRtcRedux {
    fn default() -> Self {
        glib::Object::new(&[]).unwrap()
    }
}

unsafe impl Send for WebRtcRedux {}
unsafe impl Sync for WebRtcRedux {}

//TODO: Add signal for those methods for compatibility with other programing languages
impl WebRtcRedux {
    pub fn add_ice_servers(&self, ice_servers: Vec<RTCIceServer>) {
        imp::WebRtcRedux::from_instance(self).add_ice_servers(ice_servers);
    }

    pub fn set_bundle_policy(&self, bundle_policy: RTCBundlePolicy) {
        imp::WebRtcRedux::from_instance(self).set_bundle_policy(bundle_policy);
    }

    pub fn set_sdp_semantics(&self, sdp_semantics: RTCSdpSemantics) {
        imp::WebRtcRedux::from_instance(self).set_sdp_semantics(sdp_semantics);
    }

    pub fn set_stream_id(&self, pad_name: &str, stream_id: &str) -> Result<(), ErrorMessage> {
        imp::WebRtcRedux::from_instance(self).set_stream_id(pad_name, stream_id)
    }

    pub async fn add_transceiver(
        &self,
        codec_type: RTPCodecType,
        init_params: &[RTCRtpTransceiverInit]) -> Result<Arc<RTCRtpTransceiver>, ErrorMessage>
    {
        imp::WebRtcRedux::from_instance(self)
            .add_transceiver_from_kind(codec_type, init_params)
            .await
    }

    pub async fn create_offer(
        &self,
        options: Option<RTCOfferOptions>,
    ) -> Result<SDP, ErrorMessage> {
        imp::WebRtcRedux::from_instance(self)
            .create_offer(options)
            .await
    }

    pub async fn gathering_complete_promise(&self) -> Result<tokio::sync::mpsc::Receiver<()>, ErrorMessage> {
        imp::WebRtcRedux::from_instance(self).gathering_complete_promise().await
    }

    pub async fn create_answer(
        &self,
        options: Option<RTCAnswerOptions>
    ) -> Result<SDP, ErrorMessage> {
        imp::WebRtcRedux::from_instance(self)
            .create_answer(options)
            .await
    }

    pub async fn local_description(&self) -> Result<Option<SDP>, ErrorMessage> {
        imp::WebRtcRedux::from_instance(self).local_description().await
    }

    pub async fn set_local_description(&self, sdp: &SDP, sdp_type: RTCSdpType) -> Result<(), ErrorMessage> {
        imp::WebRtcRedux::from_instance(self)
            .set_local_description(sdp, sdp_type)
            .await
    }

    pub async fn remote_description(&self) -> Result<Option<SDP>, ErrorMessage> {
        imp::WebRtcRedux::from_instance(self).remote_description().await
    }

    pub async fn set_remote_description(&self, sdp: &SDP, sdp_type: RTCSdpType) -> Result<(), ErrorMessage> {
        imp::WebRtcRedux::from_instance(self)
            .set_remote_description(sdp, sdp_type)
            .await
    }

    pub async fn on_negotiation_needed(&self, f: OnNegotiationNeededHdlrFn) -> Result<(), ErrorMessage>
    {
        imp::WebRtcRedux::from_instance(self)
            .on_negotiation_needed(f)
            .await
    }

    pub async fn on_ice_candidate(&self, f: OnLocalCandidateHdlrFn) -> Result<(), ErrorMessage>
    {
        imp::WebRtcRedux::from_instance(self)
            .on_ice_candidate(f)
            .await
    }

    pub async fn on_ice_gathering_state_change(&self, f: OnICEGathererStateChangeHdlrFn) -> Result<(), ErrorMessage>
    {
        imp::WebRtcRedux::from_instance(self)
            .on_ice_gathering_state_change(f)
            .await
    }

    pub async fn on_ice_connection_state_change(&self, f: OnICEConnectionStateChangeHdlrFn) -> Result<(), ErrorMessage>
    {
        imp::WebRtcRedux::from_instance(self)
            .on_ice_connection_state_change(f)
            .await
    }

    pub async fn on_peer_connection_state_change(&self, f: OnPeerConnectionStateChangeHdlrFn) -> Result<(), ErrorMessage>
    {
        imp::WebRtcRedux::from_instance(self)
            .on_peer_connection_state_change(f)
            .await
    }

    pub async fn add_ice_candidate(
        &self,
        candidate: RTCIceCandidateInit,
    ) -> Result<(), ErrorMessage> {
        imp::WebRtcRedux::from_instance(self)
            .add_ice_candidate(candidate)
            .await
    }

    pub async fn create_data_channel(&self, name: &str, init_params: Option<RTCDataChannelInit>) -> Result<Arc<RTCDataChannel>, ErrorMessage> {
        imp::WebRtcRedux::from_instance(self)
            .create_data_channel(name, init_params)
            .await
    }

    pub fn set_tokio_runtime(&self, handle: Handle) {
        imp::WebRtcRedux::from_instance(self).set_tokio_runtime(handle);
    }

    pub async fn wait_for_all_tracks(&self) {
        imp::WebRtcRedux::from_instance(self).wait_for_all_tracks().await;
    }
}

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    gst::Element::register(
        Some(plugin),
        "webrtcredux",
        gst::Rank::None,
        WebRtcRedux::static_type(),
    )
}
