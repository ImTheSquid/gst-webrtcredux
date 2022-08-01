use std::sync::Mutex;

use gst::{Buffer, FlowError, FlowSuccess, glib, trace};
use gst::subclass::ElementMetadata;
use gst::subclass::prelude::*;
use gst_base::subclass::prelude::*;
use once_cell::sync::Lazy;

use crate::webrtcredux::CAT;

#[derive(Default)]
struct State {}

#[derive(Default)]
pub struct WebRtcReduxSender {
    state: Mutex<State>,
}

impl ElementImpl for WebRtcReduxSender {
    fn metadata() -> Option<&'static ElementMetadata> {
        static ELEMENT_METADATA: Lazy<gst::subclass::ElementMetadata> = Lazy::new(|| {
            gst::subclass::ElementMetadata::new(
                "WebRTC Broadcast Engine (Internal sender)",
                "Sink/Video/Audio",
                "Internal WebRtcRedux sender",
                "Jack Hogan; Lorenzo Rizzotti <dev@dreaming.codes>",
            )
        });

        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: Lazy<Vec<gst::PadTemplate>> = Lazy::new(|| {
            let caps = gst::Caps::builder_full()
                .structure(gst::Structure::builder("audio/x-opus").build())
                .structure(gst::Structure::builder("audio/G722").build())
                .structure(gst::Structure::builder("audio/x-mulaw").build())
                .structure(gst::Structure::builder("audio/x-alaw").build())
                .structure(gst::Structure::builder("video/x-h264").build())
                .structure(gst::Structure::builder("video/x-vp8").build())
                .structure(gst::Structure::builder("video/x-vp9").build())
                .build();
            let sink_pad_template = gst::PadTemplate::new(
                "sink",
                gst::PadDirection::Sink,
                gst::PadPresence::Always,
                &caps,
            )
                .unwrap();

            vec![sink_pad_template]
        });

        PAD_TEMPLATES.as_ref()
    }
}

impl BaseSinkImpl for WebRtcReduxSender {
    fn render(&self, element: &Self::Type, buffer: &Buffer) -> Result<FlowSuccess, FlowError> {
        trace!(CAT, "rendering");
        Ok(gst::FlowSuccess::Ok)
    }
}

#[glib::object_subclass]
impl ObjectSubclass for WebRtcReduxSender {
    const NAME: &'static str = "WebRtcReduxSender";
    type Type = super::WebRtcReduxSender;
    type ParentType = gst_base::BaseSink;
}

impl ObjectImpl for WebRtcReduxSender {}

impl GstObjectImpl for WebRtcReduxSender {}