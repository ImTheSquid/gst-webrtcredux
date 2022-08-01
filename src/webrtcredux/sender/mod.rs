use gst::glib;

mod imp;

pub use imp::*;

glib::wrapper! {
    pub struct WebRtcReduxSender(ObjectSubclass<imp::WebRtcReduxSender>) @extends gst_base::BaseSink, gst::Element, gst::Object;
}

impl Default for WebRtcReduxSender {
    fn default() -> Self {
        glib::Object::new(&[]).unwrap()
    }
}