use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::ObjectSubclassExt;
use gst::ErrorMessage;

mod imp;

pub use imp::RTCIceServer;
pub use webrtc::peer_connection::offer_answer_options::RTCOfferOptions;

use self::sdp::SDP;
pub mod sdp;

glib::wrapper! {
    pub struct WebRtcRedux(ObjectSubclass<imp::WebRtcRedux>) @extends gst_base::BaseTransform, gst::Element, gst::Object;
}

impl Default for WebRtcRedux {
    fn default() -> Self {
        glib::Object::new(&[]).unwrap()
    }
}

//TODO: Add signal for those methods for compatibility with other programing languages
impl WebRtcRedux {
    pub fn add_ice_servers(&self, ice_servers: Vec<RTCIceServer>) {
        imp::WebRtcRedux::from_instance(self).add_ice_servers(ice_servers);
    }

    pub fn set_stream_id(&self, pad_name: &str, stream_id: &str) -> Result<(), ErrorMessage> {
        imp::WebRtcRedux::from_instance(self).set_stream_id(pad_name, stream_id)
    }

    pub async fn create_offer(
        &self,
        options: Option<RTCOfferOptions>,
    ) -> Result<SDP, ErrorMessage> {
        imp::WebRtcRedux::from_instance(&self)
            .create_offer(options)
            .await
    }

    pub async fn set_local_description(&self, sdp: &SDP) -> Result<(), ErrorMessage> {
        imp::WebRtcRedux::from_instance(&self)
            .set_local_description(sdp)
            .await
    }

    pub async fn set_remote_description(&self, sdp: &SDP) -> Result<(), ErrorMessage> {
        imp::WebRtcRedux::from_instance(&self)
            .set_remote_description(sdp)
            .await
    }
}

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    gst::Element::register(
        Some(plugin),
        "webrtcredux",
        gst::Rank::Primary,
        WebRtcRedux::static_type(),
    )
}
