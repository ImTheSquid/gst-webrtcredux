use std::sync::{Mutex, Arc};
use std::time::Duration;

use bytes::Bytes;
use futures::executor::block_on;
use gst::prelude::ClockExtManual;
use gst::traits::{ClockExt, ElementExt};
use gst::{Buffer, FlowError, FlowSuccess, glib, trace, ClockTime, debug, error};
use gst::subclass::ElementMetadata;
use gst::subclass::prelude::*;
use gst_base::subclass::prelude::*;
use once_cell::sync::Lazy;
use tokio::runtime::Handle;
use webrtc::media::Sample;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use crate::webrtcredux::CAT;

#[derive(PartialEq, Eq)]
pub enum MediaType {
    Video,
    Audio
}

#[derive(Default)]
struct State {
    track: Option<Arc<TrackLocalStaticSample>>,
    duration: Option<ClockTime>,
    handle: Option<Handle>,
    media_type: Option<MediaType>,
    async_complete: bool
}

#[derive(Default)]
pub struct WebRtcReduxSender {
    state: Arc<Mutex<State>>,
}

impl WebRtcReduxSender {
    pub fn add_info(&self, track: Arc<TrackLocalStaticSample>, handle: Handle, media_type: MediaType, duration: Option<ClockTime>, on_connect: tokio::sync::oneshot::Receiver<()>) {
        let _ = self.state.lock().unwrap().track.insert(track);
        let _ = self.state.lock().unwrap().media_type.insert(media_type);
        self.state.lock().unwrap().duration = duration;

        let instance = self.instance().clone();
        let state = self.state.clone();
        handle.spawn(async move {
            if on_connect.await.is_err() { error!(CAT, "Error waiting for peer connection"); return; }
            state.lock().unwrap().async_complete = true;
            debug!(CAT, "Peer connection successful, finishing async transition");
            instance.change_state(gst::StateChange::PausedToPlaying).unwrap();
        });
        let _ = self.state.lock().unwrap().handle.insert(handle);
    }
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
                .structure(gst::Structure::builder("video/x-h264").field("stream-format", "byte-stream").field("profile", "baseline").build())
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

    fn change_state(&self, transition: gst::StateChange) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
        if transition == gst::StateChange::PausedToPlaying {
            if let Some(duration) = self.state.lock().unwrap().duration {
                self.set_clock(Some(&format_clock(duration)));
            }

            if !self.state.lock().unwrap().async_complete {
                return Ok(gst::StateChangeSuccess::Async);
            }
        }
        self.parent_change_state(transition)
    }
}

impl BaseSinkImpl for WebRtcReduxSender {
    fn render(&self, buffer: &Buffer) -> Result<FlowSuccess, FlowError> {
        let sample_duration = if *self.state.lock().unwrap().media_type.as_ref().unwrap() == MediaType::Video {
            Duration::from_secs(1)
        } else {
            Duration::from_millis(buffer.duration().unwrap().mseconds())
        };

        // If the clock hasn't been set, set it from the buffer timestamp
        if self.state.lock().unwrap().duration.is_none() {
            let _ = self.state.lock().unwrap().duration.insert(buffer.duration().unwrap());
            self.set_clock(Some(&format_clock(buffer.duration().unwrap())));
        }

        let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
        trace!(CAT, "Rendering {} bytes", map.size());
        let bytes = Bytes::copy_from_slice(map.as_slice());

        let handle = self.state.lock().unwrap().handle.as_ref().unwrap().clone();
        let track = self.state.lock().unwrap().track.as_ref().unwrap().clone();
        let inner = handle.clone();
        block_on(async move {
            handle.spawn_blocking(move || {
                inner.block_on(async move {
                    track.write_sample(&Sample {
                        data: bytes,
                        duration: sample_duration,
                        ..Sample::default()
                    }).await
                })
            }).await
        }).unwrap().unwrap();

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

fn format_clock(duration: ClockTime) -> gst::Clock {
    let clock = gst::SystemClock::obtain();
    let _ = clock.new_periodic_id(clock.internal_time(), duration);

    clock
}