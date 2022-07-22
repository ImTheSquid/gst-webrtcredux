use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::ObjectSubclassExt;

mod imp;

pub use imp::RTCIceServer;

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
}

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    gst::Element::register(Some(plugin), "webrtcredux", gst::Rank::Primary, WebRtcRedux::static_type())
}