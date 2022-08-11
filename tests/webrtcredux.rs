use std::fmt::{Display, Formatter};
use std::str::FromStr;

use enum_dispatch::enum_dispatch;
use gst::glib::BoolError;
use gst::prelude::*;
use gst::{debug_bin_to_dot_data, DebugGraphDetails, Element};
use indoc::indoc;
use webrtcredux::sdp::LineEnding;
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

    }

    pipeline.set_state(gst::State::Playing).expect("Failed to set pipeline state");

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
    a=rtpmap:99 h263-1998/90000
    ");

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

    assert_eq!(test.to_string(LineEnding::LF), target);
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

#[test]
fn complex_sdp() {
    let text = indoc!("v=0
    o=- 8488083020976882093 2 IN IP4 127.0.0.1
    s=-
    t=0 0
    a=group:BUNDLE 0 1
    a=extmap-allow-mixed
    a=msid-semantic: WMS
    m=video 55395 UDP/TLS/RTP/SAVPF 96 97 98 99 100 101 127 121 125 107 108 109 124 120 123 119 35 36 41 42 114 115 116
    c=IN IP4 2.39.73.41
    a=rtcp:9 IN IP4 0.0.0.0
    a=candidate:3859917557 1 udp 2113937151 44a9eba8-5284-45b5-8825-ed5f7001f62a.local 55395 typ host generation 0 network-cost 999
    a=candidate:842163049 1 udp 1677729535 2.39.73.41 55395 typ srflx raddr 0.0.0.0 rport 0 generation 0 network-cost 999
    a=ice-ufrag:nVwA
    a=ice-pwd:tyR7PZVvcMN4/aqQLrcBFuU5
    a=ice-options:trickle
    a=fingerprint:sha-256 62:E4:9A:F9:6A:F5:B4:E3:52:07:4F:8E:C4:9F:27:16:9B:DA:D1:18:00:19:5F:8A:69:E2:D9:F6:AC:F0:64:51
    a=setup:actpass
    a=mid:0
    a=extmap:1 urn:ietf:params:rtp-hdrext:toffset
    a=extmap:2 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time
    a=extmap:3 urn:3gpp:video-orientation
    a=extmap:4 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01
    a=extmap:5 http://www.webrtc.org/experiments/rtp-hdrext/playout-delay
    a=extmap:6 http://www.webrtc.org/experiments/rtp-hdrext/video-content-type
    a=extmap:7 http://www.webrtc.org/experiments/rtp-hdrext/video-timing
    a=extmap:8 http://www.webrtc.org/experiments/rtp-hdrext/color-space
    a=extmap:9 urn:ietf:params:rtp-hdrext:sdes:mid
    a=extmap:10 urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id
    a=extmap:11 urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id
    a=sendrecv
    a=msid:- aef93e5f-0aeb-4c4d-807e-fadaf721fc63
    a=rtcp-mux
    a=rtcp-rsize
    a=rtpmap:96 VP8/90000
    a=rtcp-fb:96 goog-remb
    a=rtcp-fb:96 transport-cc
    a=rtcp-fb:96 ccm fir
    a=rtcp-fb:96 nack
    a=rtcp-fb:96 nack pli
    a=rtpmap:97 rtx/90000
    a=fmtp:97 apt=96
    a=rtpmap:98 VP9/90000
    a=rtcp-fb:98 goog-remb
    a=rtcp-fb:98 transport-cc
    a=rtcp-fb:98 ccm fir
    a=rtcp-fb:98 nack
    a=rtcp-fb:98 nack pli
    a=fmtp:98 profile-id=0
    a=rtpmap:99 rtx/90000
    a=fmtp:99 apt=98
    a=rtpmap:100 VP9/90000
    a=rtcp-fb:100 goog-remb
    a=rtcp-fb:100 transport-cc
    a=rtcp-fb:100 ccm fir
    a=rtcp-fb:100 nack
    a=rtcp-fb:100 nack pli
    a=fmtp:100 profile-id=2
    a=rtpmap:101 rtx/90000
    a=fmtp:101 apt=100
    a=rtpmap:127 H264/90000
    a=rtcp-fb:127 goog-remb
    a=rtcp-fb:127 transport-cc
    a=rtcp-fb:127 ccm fir
    a=rtcp-fb:127 nack
    a=rtcp-fb:127 nack pli
    a=fmtp:127 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f
    a=rtpmap:121 rtx/90000
    a=fmtp:121 apt=127
    a=rtpmap:125 H264/90000
    a=rtcp-fb:125 goog-remb
    a=rtcp-fb:125 transport-cc
    a=rtcp-fb:125 ccm fir
    a=rtcp-fb:125 nack
    a=rtcp-fb:125 nack pli
    a=fmtp:125 level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=42001f
    a=rtpmap:107 rtx/90000
    a=fmtp:107 apt=125
    a=rtpmap:108 H264/90000
    a=rtcp-fb:108 goog-remb
    a=rtcp-fb:108 transport-cc
    a=rtcp-fb:108 ccm fir
    a=rtcp-fb:108 nack
    a=rtcp-fb:108 nack pli
    a=fmtp:108 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f
    a=rtpmap:109 rtx/90000
    a=fmtp:109 apt=108
    a=rtpmap:124 H264/90000
    a=rtcp-fb:124 goog-remb
    a=rtcp-fb:124 transport-cc
    a=rtcp-fb:124 ccm fir
    a=rtcp-fb:124 nack
    a=rtcp-fb:124 nack pli
    a=fmtp:124 level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=42e01f
    a=rtpmap:120 rtx/90000
    a=fmtp:120 apt=124
    a=rtpmap:123 H264/90000
    a=rtcp-fb:123 goog-remb
    a=rtcp-fb:123 transport-cc
    a=rtcp-fb:123 ccm fir
    a=rtcp-fb:123 nack
    a=rtcp-fb:123 nack pli
    a=fmtp:123 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=4d001f
    a=rtpmap:119 rtx/90000
    a=fmtp:119 apt=123
    a=rtpmap:35 H264/90000
    a=rtcp-fb:35 goog-remb
    a=rtcp-fb:35 transport-cc
    a=rtcp-fb:35 ccm fir
    a=rtcp-fb:35 nack
    a=rtcp-fb:35 nack pli
    a=fmtp:35 level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=4d001f
    a=rtpmap:36 rtx/90000
    a=fmtp:36 apt=35
    a=rtpmap:41 AV1/90000
    a=rtcp-fb:41 goog-remb
    a=rtcp-fb:41 transport-cc
    a=rtcp-fb:41 ccm fir
    a=rtcp-fb:41 nack
    a=rtcp-fb:41 nack pli
    a=rtpmap:42 rtx/90000
    a=fmtp:42 apt=41
    a=rtpmap:114 red/90000
    a=rtpmap:115 rtx/90000
    a=fmtp:115 apt=114
    a=rtpmap:116 ulpfec/90000
    a=ssrc-group:FID 2188188946 3056071260
    a=ssrc:2188188946 cname:QGl7AJpaZdNMdnjK
    a=ssrc:2188188946 msid:- aef93e5f-0aeb-4c4d-807e-fadaf721fc63
    a=ssrc:3056071260 cname:QGl7AJpaZdNMdnjK
    a=ssrc:3056071260 msid:- aef93e5f-0aeb-4c4d-807e-fadaf721fc63
    m=audio 34179 UDP/TLS/RTP/SAVPF 111 63 103 104 9 0 8 106 105 13 110 112 113 126
    c=IN IP4 2.39.73.41
    a=rtcp:9 IN IP4 0.0.0.0
    a=candidate:3859917557 1 udp 2113937151 44a9eba8-5284-45b5-8825-ed5f7001f62a.local 34179 typ host generation 0 network-cost 999
    a=candidate:842163049 1 udp 1677729535 2.39.73.41 34179 typ srflx raddr 0.0.0.0 rport 0 generation 0 network-cost 999
    a=ice-ufrag:nVwA
    a=ice-pwd:tyR7PZVvcMN4/aqQLrcBFuU5
    a=ice-options:trickle
    a=fingerprint:sha-256 62:E4:9A:F9:6A:F5:B4:E3:52:07:4F:8E:C4:9F:27:16:9B:DA:D1:18:00:19:5F:8A:69:E2:D9:F6:AC:F0:64:51
    a=setup:actpass
    a=mid:1
    a=extmap:14 urn:ietf:params:rtp-hdrext:ssrc-audio-level
    a=extmap:2 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time
    a=extmap:4 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01
    a=extmap:9 urn:ietf:params:rtp-hdrext:sdes:mid
    a=sendrecv
    a=msid:- c8351fd3-2f5d-4d46-899d-9af77de86d9b
    a=rtcp-mux
    a=rtpmap:111 opus/48000/2
    a=rtcp-fb:111 transport-cc
    a=fmtp:111 minptime=10;useinbandfec=1
    a=rtpmap:63 red/48000/2
    a=fmtp:63 111/111
    a=rtpmap:103 ISAC/16000
    a=rtpmap:104 ISAC/32000
    a=rtpmap:9 G722/8000
    a=rtpmap:0 PCMU/8000
    a=rtpmap:8 PCMA/8000
    a=rtpmap:106 CN/32000
    a=rtpmap:105 CN/16000
    a=rtpmap:13 CN/8000
    a=rtpmap:110 telephone-event/48000
    a=rtpmap:112 telephone-event/32000
    a=rtpmap:113 telephone-event/16000
    a=rtpmap:126 telephone-event/8000
    a=ssrc:3846141828 cname:QGl7AJpaZdNMdnjK
    a=ssrc:3846141828 msid:- c8351fd3-2f5d-4d46-899d-9af77de86d9b");

    let sdp = SDP::from_str(text);
    
    assert!(sdp.is_ok());
}

#[test]
fn complex_unformatted_sdp() {
    let text = "v=0\r\no=- 8488083020976882093 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\na=group:BUNDLE 0 1\r\na=extmap-allow-mixed\r\na=msid-semantic: WMS\r\nm=video 55395 UDP/TLS/RTP/SAVPF 96 97 98 99 100 101 127 121 125 107 108 109 124 120 123 119 35 36 41 42 114 115 116\r\nc=IN IP4 2.39.73.41\r\na=rtcp:9 IN IP4 0.0.0.0\r\na=candidate:3859917557 1 udp 2113937151 44a9eba8-5284-45b5-8825-ed5f7001f62a.local 55395 typ host generation 0 network-cost 999\r\na=candidate:842163049 1 udp 1677729535 2.39.73.41 55395 typ srflx raddr 0.0.0.0 rport 0 generation 0 network-cost 999\r\na=ice-ufrag:nVwA\r\na=ice-pwd:tyR7PZVvcMN4/aqQLrcBFuU5\r\na=ice-options:trickle\r\na=fingerprint:sha-256 62:E4:9A:F9:6A:F5:B4:E3:52:07:4F:8E:C4:9F:27:16:9B:DA:D1:18:00:19:5F:8A:69:E2:D9:F6:AC:F0:64:51\r\na=setup:actpass\r\na=mid:0\r\na=extmap:1 urn:ietf:params:rtp-hdrext:toffset\r\na=extmap:2 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time\r\na=extmap:3 urn:3gpp:video-orientation\r\na=extmap:4 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01\r\na=extmap:5 http://www.webrtc.org/experiments/rtp-hdrext/playout-delay\r\na=extmap:6 http://www.webrtc.org/experiments/rtp-hdrext/video-content-type\r\na=extmap:7 http://www.webrtc.org/experiments/rtp-hdrext/video-timing\r\na=extmap:8 http://www.webrtc.org/experiments/rtp-hdrext/color-space\r\na=extmap:9 urn:ietf:params:rtp-hdrext:sdes:mid\r\na=extmap:10 urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id\r\na=extmap:11 urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id\r\na=sendrecv\r\na=msid:- aef93e5f-0aeb-4c4d-807e-fadaf721fc63\r\na=rtcp-mux\r\na=rtcp-rsize\r\na=rtpmap:96 VP8/90000\r\na=rtcp-fb:96 goog-remb\r\na=rtcp-fb:96 transport-cc\r\na=rtcp-fb:96 ccm fir\r\na=rtcp-fb:96 nack\r\na=rtcp-fb:96 nack pli\r\na=rtpmap:97 rtx/90000\r\na=fmtp:97 apt=96\r\na=rtpmap:98 VP9/90000\r\na=rtcp-fb:98 goog-remb\r\na=rtcp-fb:98 transport-cc\r\na=rtcp-fb:98 ccm fir\r\na=rtcp-fb:98 nack\r\na=rtcp-fb:98 nack pli\r\na=fmtp:98 profile-id=0\r\na=rtpmap:99 rtx/90000\r\na=fmtp:99 apt=98\r\na=rtpmap:100 VP9/90000\r\na=rtcp-fb:100 goog-remb\r\na=rtcp-fb:100 transport-cc\r\na=rtcp-fb:100 ccm fir\r\na=rtcp-fb:100 nack\r\na=rtcp-fb:100 nack pli\r\na=fmtp:100 profile-id=2\r\na=rtpmap:101 rtx/90000\r\na=fmtp:101 apt=100\r\na=rtpmap:127 H264/90000\r\na=rtcp-fb:127 goog-remb\r\na=rtcp-fb:127 transport-cc\r\na=rtcp-fb:127 ccm fir\r\na=rtcp-fb:127 nack\r\na=rtcp-fb:127 nack pli\r\na=fmtp:127 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f\r\na=rtpmap:121 rtx/90000\r\na=fmtp:121 apt=127\r\na=rtpmap:125 H264/90000\r\na=rtcp-fb:125 goog-remb\r\na=rtcp-fb:125 transport-cc\r\na=rtcp-fb:125 ccm fir\r\na=rtcp-fb:125 nack\r\na=rtcp-fb:125 nack pli\r\na=fmtp:125 level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=42001f\r\na=rtpmap:107 rtx/90000\r\na=fmtp:107 apt=125\r\na=rtpmap:108 H264/90000\r\na=rtcp-fb:108 goog-remb\r\na=rtcp-fb:108 transport-cc\r\na=rtcp-fb:108 ccm fir\r\na=rtcp-fb:108 nack\r\na=rtcp-fb:108 nack pli\r\na=fmtp:108 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f\r\na=rtpmap:109 rtx/90000\r\na=fmtp:109 apt=108\r\na=rtpmap:124 H264/90000\r\na=rtcp-fb:124 goog-remb\r\na=rtcp-fb:124 transport-cc\r\na=rtcp-fb:124 ccm fir\r\na=rtcp-fb:124 nack\r\na=rtcp-fb:124 nack pli\r\na=fmtp:124 level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=42e01f\r\na=rtpmap:120 rtx/90000\r\na=fmtp:120 apt=124\r\na=rtpmap:123 H264/90000\r\na=rtcp-fb:123 goog-remb\r\na=rtcp-fb:123 transport-cc\r\na=rtcp-fb:123 ccm fir\r\na=rtcp-fb:123 nack\r\na=rtcp-fb:123 nack pli\r\na=fmtp:123 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=4d001f\r\na=rtpmap:119 rtx/90000\r\na=fmtp:119 apt=123\r\na=rtpmap:35 H264/90000\r\na=rtcp-fb:35 goog-remb\r\na=rtcp-fb:35 transport-cc\r\na=rtcp-fb:35 ccm fir\r\na=rtcp-fb:35 nack\r\na=rtcp-fb:35 nack pli\r\na=fmtp:35 level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=4d001f\r\na=rtpmap:36 rtx/90000\r\na=fmtp:36 apt=35\r\na=rtpmap:41 AV1/90000\r\na=rtcp-fb:41 goog-remb\r\na=rtcp-fb:41 transport-cc\r\na=rtcp-fb:41 ccm fir\r\na=rtcp-fb:41 nack\r\na=rtcp-fb:41 nack pli\r\na=rtpmap:42 rtx/90000\r\na=fmtp:42 apt=41\r\na=rtpmap:114 red/90000\r\na=rtpmap:115 rtx/90000\r\na=fmtp:115 apt=114\r\na=rtpmap:116 ulpfec/90000\r\na=ssrc-group:FID 2188188946 3056071260\r\na=ssrc:2188188946 cname:QGl7AJpaZdNMdnjK\r\na=ssrc:2188188946 msid:- aef93e5f-0aeb-4c4d-807e-fadaf721fc63\r\na=ssrc:3056071260 cname:QGl7AJpaZdNMdnjK\r\na=ssrc:3056071260 msid:- aef93e5f-0aeb-4c4d-807e-fadaf721fc63\r\nm=audio 34179 UDP/TLS/RTP/SAVPF 111 63 103 104 9 0 8 106 105 13 110 112 113 126\r\nc=IN IP4 2.39.73.41\r\na=rtcp:9 IN IP4 0.0.0.0\r\na=candidate:3859917557 1 udp 2113937151 44a9eba8-5284-45b5-8825-ed5f7001f62a.local 34179 typ host generation 0 network-cost 999\r\na=candidate:842163049 1 udp 1677729535 2.39.73.41 34179 typ srflx raddr 0.0.0.0 rport 0 generation 0 network-cost 999\r\na=ice-ufrag:nVwA\r\na=ice-pwd:tyR7PZVvcMN4/aqQLrcBFuU5\r\na=ice-options:trickle\r\na=fingerprint:sha-256 62:E4:9A:F9:6A:F5:B4:E3:52:07:4F:8E:C4:9F:27:16:9B:DA:D1:18:00:19:5F:8A:69:E2:D9:F6:AC:F0:64:51\r\na=setup:actpass\r\na=mid:1\r\na=extmap:14 urn:ietf:params:rtp-hdrext:ssrc-audio-level\r\na=extmap:2 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time\r\na=extmap:4 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01\r\na=extmap:9 urn:ietf:params:rtp-hdrext:sdes:mid\r\na=sendrecv\r\na=msid:- c8351fd3-2f5d-4d46-899d-9af77de86d9b\r\na=rtcp-mux\r\na=rtpmap:111 opus/48000/2\r\na=rtcp-fb:111 transport-cc\r\na=fmtp:111 minptime=10;useinbandfec=1\r\na=rtpmap:63 red/48000/2\r\na=fmtp:63 111/111\r\na=rtpmap:103 ISAC/16000\r\na=rtpmap:104 ISAC/32000\r\na=rtpmap:9 G722/8000\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:8 PCMA/8000\r\na=rtpmap:106 CN/32000\r\na=rtpmap:105 CN/16000\r\na=rtpmap:13 CN/8000\r\na=rtpmap:110 telephone-event/48000\r\na=rtpmap:112 telephone-event/32000\r\na=rtpmap:113 telephone-event/16000\r\na=rtpmap:126 telephone-event/8000\r\na=ssrc:3846141828 cname:QGl7AJpaZdNMdnjK\r\na=ssrc:3846141828 msid:- c8351fd3-2f5d-4d46-899d-9af77de86d9b\r\n";

    let sdp = SDP::from_str(text);

    assert!(sdp.is_ok());
}

#[test]
fn sdp_symmetry() {
    let text = "v=0\r\no=- 9023059822302806521 801820409 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\na=fingerprint:sha-256 18:FB:AD:1F:CC:77:25:4F:6C:CD:3F:88:58:94:26:D5:B3:9B:72:CB:5A:9A:0E:A0:5D:C4:C8:E3:1D:5A:5A:6D\r\na=group:BUNDLE\r\nm=video 0 UDP/TLS/RTP/SAVPF 0\r\nm=audio 0 UDP/TLS/RTP/SAVPF 0\r\n";

    let sdp = SDP::from_str(text);

    assert!(sdp.is_ok());

    assert_eq!(text, sdp.unwrap().to_string(LineEnding::CRLF));
}
