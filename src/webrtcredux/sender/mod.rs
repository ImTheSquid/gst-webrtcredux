use std::{sync::{Mutex, Arc}, time::Duration};

use gst::glib;
use gst::subclass::prelude::ObjectSubclassExt;

mod imp;

pub use imp::*;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

glib::wrapper! {
    pub struct WebRtcReduxSender(ObjectSubclass<imp::WebRtcReduxSender>) @extends gst_base::BaseSink, gst::Element, gst::Object;
}

impl WebRtcReduxSender {
    pub fn add_info(&self, track: Arc<TrackLocalStaticSample>, duration: Option<Duration>) {
        imp::WebRtcReduxSender::from_instance(self).add_info(track, duration);
    }
}

impl Default for WebRtcReduxSender {
    fn default() -> Self {
        glib::Object::new(&[]).unwrap()
    }
}