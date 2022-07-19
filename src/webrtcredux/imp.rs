use gst::glib;
use gst_base::subclass::prelude::*;

use std::i32;
use std::sync::Mutex;

use once_cell::sync::Lazy;

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        "webrtcbin",
        gst::DebugColorFlags::empty(),
        Some("WebRTC Video and Audio Transmitter"),
    )
});

#[derive(Default)]
pub struct WebRtcRedux {}

impl WebRtcRedux {}

#[glib::object_subclass]
impl ObjectSubclass for WebRtcRedux {
    const NAME: &'static str = "WebRtcRedux";
    type Type = super::WebRtcRedux;
    type ParentType = gst_base::BaseTransform;
}

impl ObjectImpl for WebRtcRedux {}
impl ElementImpl for WebRtcRedux {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: Lazy<gst::subclass::ElementMetadata> = Lazy::new(|| {
            gst::subclass::ElementMetadata::new(
                "WebRTC Broadcast Engine",
            "Sink/Video/Audio",
            "Broadcasts encoded video and audio",
            "Jack Hogan"
            )
        });

        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: Lazy<Vec<gst::PadTemplate>> = Lazy::new(|| {
            let mut base = gst::Caps::new_empty_simple("video/x-h264");
            let video_caps = base.get_mut().unwrap();
            video_caps.append(gst::Caps::new_empty_simple("video/VP8"));
            video_caps.append(gst::Caps::new_empty_simple("video/VP9"));

            let video_sink = gst::PadTemplate::new(
                "video", 
                gst::PadDirection::Sink, 
                gst::PadPresence::Always,
                &video_caps.to_owned()
            ).unwrap();

            let audio_caps = gst::Caps::new_empty_simple("audio/x-opus");

            let audio_sink = gst::PadTemplate::new(
                "audio", 
                gst::PadDirection::Sink, 
                gst::PadPresence::Always,
                &audio_caps
            ).unwrap();


            vec![video_sink, audio_sink]
        });

        PAD_TEMPLATES.as_ref()
    }
}

impl GstObjectImpl for WebRtcRedux {}

impl BaseTransformImpl for WebRtcRedux {
    const MODE: gst_base::subclass::BaseTransformMode =
        gst_base::subclass::BaseTransformMode::NeverInPlace;
    const PASSTHROUGH_ON_SAME_CAPS: bool = false;
    const TRANSFORM_IP_ON_PASSTHROUGH: bool = false;

    fn set_caps(&self, element: &Self::Type, incaps: &gst::Caps, outcaps: &gst::Caps) -> Result<(), gst::LoggableError> {
        Ok(())
    }

    fn stop(&self, element: &Self::Type) -> Result<(), gst::ErrorMessage> {
        Ok(())
    }

    fn unit_size(&self, element: &Self::Type, caps: &gst::Caps) -> Option<usize> {
        None
    }

    fn transform_caps(&self, element: &Self::Type, direction: gst::PadDirection, caps: &gst::Caps, filter: Option<&gst::Caps>) -> Option<gst::Caps> {
        None
    }
}