use std::fmt::{Display, Formatter};
use std::str::FromStr;

use enum_dispatch::enum_dispatch;
use gst::glib::BoolError;
use gst::prelude::*;
use gst::{debug_bin_to_dot_data, DebugGraphDetails, Element};
use indoc::indoc;
use std::string::ToString;
use strum::IntoEnumIterator;
use strum_macros::Display;
use strum_macros::EnumIter;

use webrtcredux::webrtcredux::{
    sdp::{AddressType, MediaProp, MediaType, NetworkType, SdpProp, SDP},
    RTCIceServer, WebRtcRedux,
};

//TODO: Implement a webrtc-rs server configured for receiving to test the plugin

fn init() {
    use std::sync::Once;
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        gst::init().unwrap();
        webrtcredux::plugin_register_static().unwrap();
    })
}

fn init_tests_dir() {
    use std::sync::Once;
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let _ = std::fs::remove_dir_all("./target/debug/tests");
        std::fs::create_dir_all("./target/debug/tests").expect("Failed to create tests dir");
    })
}

#[enum_dispatch(Encoder)]
pub trait GstEncoder {
    fn to_gst_encoder(&self) -> Result<Element, BoolError>;
}

#[derive(Debug, EnumIter, Display)]
enum AudioEncoder {
    Opus,
    Mulaw,
    Alaw,
    G722,
}

impl GstEncoder for AudioEncoder {
    fn to_gst_encoder(&self) -> Result<Element, BoolError> {
        match self {
            AudioEncoder::Opus => gst::ElementFactory::make("opusenc", None),
            AudioEncoder::Mulaw => gst::ElementFactory::make("mulawenc", None),
            AudioEncoder::Alaw => gst::ElementFactory::make("alawenc", None),
            AudioEncoder::G722 => gst::ElementFactory::make("avenc_g722", None),
        }
    }
}

#[derive(Debug, EnumIter, Display)]
enum VideoEncoder {
    H264,
    VP8,
    VP9,
}

impl GstEncoder for VideoEncoder {
    fn to_gst_encoder(&self) -> Result<Element, BoolError> {
        match self {
            VideoEncoder::H264 => gst::ElementFactory::make("x264enc", None),
            VideoEncoder::VP8 => gst::ElementFactory::make("vp8enc", None),
            VideoEncoder::VP9 => gst::ElementFactory::make("vp9enc", None),
        }
    }
}

#[derive(Debug)]
#[enum_dispatch]
enum Encoder {
    Audio(AudioEncoder),
    Video(VideoEncoder),
}

impl Display for Encoder {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Encoder::Audio(encoder) => write!(f, "audio_{}", encoder.to_string()),
            Encoder::Video(encoder) => write!(f, "video_{}", encoder.to_string()),
        }
    }
}

#[test]
fn pipeline_creation_h264() {
    pipeline_creation_test(vec![Encoder::Video(VideoEncoder::H264)]);
}

#[test]
fn pipeline_creation_vp8() {
    pipeline_creation_test(vec![Encoder::Video(VideoEncoder::VP8)]);
}

#[test]
fn pipeline_creation_vp9() {
    pipeline_creation_test(vec![Encoder::Video(VideoEncoder::VP9)]);
}

#[test]
fn pipeline_creation_opus() {
    pipeline_creation_test(vec![Encoder::Audio(AudioEncoder::Opus)]);
}

#[test]
fn pipeline_creation_mulaw() {
    pipeline_creation_test(vec![Encoder::Audio(AudioEncoder::Mulaw)]);
}

#[test]
fn pipeline_creation_alaw() {
    pipeline_creation_test(vec![Encoder::Audio(AudioEncoder::Alaw)]);
}

#[test]
fn pipeline_creation_combined() {
    let mut to_test = vec![];
    for a_encoder in AudioEncoder::iter() {
        to_test.push(Encoder::Audio(a_encoder));
    }
    for v_encoder in VideoEncoder::iter() {
        to_test.push(Encoder::Video(v_encoder));
    }
    pipeline_creation_test(to_test);
}

fn pipeline_creation_test(encoders: Vec<Encoder>) {
    init();
    let pipeline = gst::Pipeline::new(None);

    let webrtcredux = WebRtcRedux::default();

    webrtcredux.add_ice_servers(vec![RTCIceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_string()],
        ..Default::default()
    }]);

    pipeline
        .add(&webrtcredux)
        .expect("Failed to add webrtcredux to the pipeline");

    let mut audio_idx: usize = 0;
    let mut video_idx: usize = 0;
    for encoder_to_use in &encoders {
        let src = match encoder_to_use {
            Encoder::Audio(_) => gst::ElementFactory::make("audiotestsrc", None).unwrap(),
            Encoder::Video(_) => gst::ElementFactory::make("videotestsrc", None).unwrap(),
        };

        let encoder = encoder_to_use.to_gst_encoder().unwrap();

        pipeline
            .add_many(&[&src, &encoder])
            .expect("Failed to add elements to the pipeline");
        Element::link_many(&[&src, &encoder, webrtcredux.as_ref()])
            .expect("Failed to link elements");

        match encoder_to_use {
            Encoder::Audio(_) => {
                let pad_name = &format!("audio_{}", audio_idx);
                webrtcredux.set_stream_id(pad_name, pad_name).unwrap();
                audio_idx += 1;
            }
            Encoder::Video(_) => {
                let pad_name = &format!("video_{}", video_idx);
                webrtcredux.set_stream_id(pad_name, pad_name).unwrap();
                video_idx += 1;
            }
        }
    }

    assert_eq!(
        pipeline.set_state(gst::State::Playing).unwrap(),
        gst::StateChangeSuccess::Success
    );

    // Debug diagram
    let out = debug_bin_to_dot_data(&pipeline, DebugGraphDetails::ALL);
    init_tests_dir();
    std::fs::write(
        format!(
            "./target/debug/tests/{}.dot",
            encoders
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("-")
        ),
        out.as_str(),
    )
    .unwrap();
}

#[test]
fn sdp_serialization() {
    let target = indoc!(
        "v=0
    o=jdoe 2890844526 2890842807 IN IP4 10.47.16.5
    s=SDP Seminar
    i=A Seminar on the session description protocol
    u=http://www.example.com/seminars/sdp.pdf
    e=j.doe@example.com (Jane Doe)
    c=IN IP4 224.2.17.12/127
    t=2873397496 2873404696
    a=recvonly
    m=audio 49170 RTP/AVP 0
    m=video 51372 RTP/AVP 99
    a=rtpmap:99 h263-1998/90000"
    );

    let props = vec![
        SdpProp::Version(0),
        SdpProp::Origin {
            username: "jdoe".to_string(),
            session_id: "2890844526".to_string(),
            session_version: 2890842807,
            net_type: NetworkType::Internet,
            address_type: AddressType::IPv4,
            address: "10.47.16.5".to_string(),
        },
        SdpProp::SessionName("SDP Seminar".to_string()),
        SdpProp::SessionInformation("A Seminar on the session description protocol".to_string()),
        SdpProp::Uri("http://www.example.com/seminars/sdp.pdf".to_string()),
        SdpProp::Email("j.doe@example.com (Jane Doe)".to_string()),
        SdpProp::Connection {
            net_type: NetworkType::Internet,
            address_type: AddressType::IPv4,
            address: "224.2.17.12".to_string(),
            suffix: None,
            ttl: Some(127),
            num_addresses: None,
        },
        SdpProp::Timing {
            start: 2873397496,
            stop: 2873404696,
        },
        SdpProp::Attribute {
            key: "recvonly".to_string(),
            value: None,
        },
        SdpProp::Media {
            r#type: MediaType::Audio,
            ports: vec![49170],
            protocol: "RTP/AVP".to_string(),
            format: "0".to_string(),
            props: vec![],
        },
        SdpProp::Media {
            r#type: MediaType::Video,
            ports: vec![51372],
            protocol: "RTP/AVP".to_string(),
            format: "99".to_string(),
            props: vec![MediaProp::Attribute {
                key: "rtpmap".to_string(),
                value: Some("99 h263-1998/90000".to_string()),
            }],
        },
    ];

    let test = SDP { props };

    assert_eq!(test.to_string(), target);
}

#[test]
fn sdp_deserialization() {
    let props = vec![
        SdpProp::Version(0),
        SdpProp::Origin {
            username: "jdoe".to_string(),
            session_id: "2890844526".to_string(),
            session_version: 2890842807,
            net_type: NetworkType::Internet,
            address_type: AddressType::IPv4,
            address: "10.47.16.5".to_string(),
        },
        SdpProp::SessionName("SDP Seminar".to_string()),
        SdpProp::SessionInformation("A Seminar on the session description protocol".to_string()),
        SdpProp::Uri("http://www.example.com/seminars/sdp.pdf".to_string()),
        SdpProp::Email("j.doe@example.com (Jane Doe)".to_string()),
        SdpProp::Connection {
            net_type: NetworkType::Internet,
            address_type: AddressType::IPv4,
            address: "224.2.17.12".to_string(),
            suffix: None,
            ttl: Some(127),
            num_addresses: None,
        },
        SdpProp::Timing {
            start: 2873397496,
            stop: 2873404696,
        },
        SdpProp::Attribute {
            key: "recvonly".to_string(),
            value: None,
        },
        SdpProp::Media {
            r#type: MediaType::Audio,
            ports: vec![49170],
            protocol: "RTP/AVP".to_string(),
            format: "0".to_string(),
            props: vec![],
        },
        SdpProp::Media {
            r#type: MediaType::Video,
            ports: vec![51372],
            protocol: "RTP/AVP".to_string(),
            format: "99".to_string(),
            props: vec![MediaProp::Attribute {
                key: "rtpmap".to_string(),
                value: Some("99 h263-1998/90000".to_string()),
            }],
        },
    ];

    let target = SDP { props };

    let test = indoc!(
        "v=0
    o=jdoe 2890844526 2890842807 IN IP4 10.47.16.5
    s=SDP Seminar
    i=A Seminar on the session description protocol
    u=http://www.example.com/seminars/sdp.pdf
    e=j.doe@example.com (Jane Doe)
    c=IN IP4 224.2.17.12/127
    t=2873397496 2873404696
    a=recvonly
    m=audio 49170 RTP/AVP 0
    m=video 51372 RTP/AVP 99
    a=rtpmap:99 h263-1998/90000"
    );

    let res = SDP::from_str(test);

    assert!(
        res.is_ok(),
        "Parse failed with error: {:?}",
        res.err().unwrap()
    );

    assert_eq!(res.unwrap(), target);
}
