use gst::glib;
use gst::prelude::*;

mod imp;

glib::wrapper! {
    pub struct WebRtcRedux(ObjectSubclass<imp::WebRtcRedux>) @extends gst_base::BaseTransform, gst::Element, gst::Object;
}

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    gst::Element::register(Some(plugin), "webrtcredux", gst::Rank::Primary, WebRtcRedux::static_type())
}