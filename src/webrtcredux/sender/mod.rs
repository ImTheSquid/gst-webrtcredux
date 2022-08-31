use std::sync::Arc;

use gst::{glib, ClockTime};
use gst::subclass::prelude::ObjectSubclassExt;

mod imp;

pub use imp::*;
use tokio::runtime::Handle;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

glib::wrapper! {
    pub struct WebRtcReduxSender(ObjectSubclass<imp::WebRtcReduxSender>) @extends gst_base::BaseSink, gst::Element, gst::Object;
}

impl WebRtcReduxSender {
    pub fn add_info(&self, track: Arc<TrackLocalStaticSample>, handle: Handle, media_type: MediaType, duration: Option<ClockTime>) {
        imp::WebRtcReduxSender::from_instance(self).add_info(track, handle, media_type, duration);
    }
}

unsafe impl Send for WebRtcReduxSender {}

impl Default for WebRtcReduxSender {
    fn default() -> Self {
        glib::Object::new(&[]).unwrap()
    }
}