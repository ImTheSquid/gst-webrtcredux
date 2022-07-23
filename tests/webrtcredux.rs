use std::str::FromStr;

use gst::prelude::*;
use webrtcredux::webrtcredux::{WebRtcRedux, sdp::{SDP, SdpProp, NetworkType, AddressType, MediaType, MediaProtocol, MediaProp}, RTCIceServer};

//TODO: Implement a webrtc-rs server configured for receiving to test the plugin

fn init() {
    use std::sync::Once;
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        gst::init().unwrap();
        webrtcredux::plugin_register_static().unwrap();
    })
}

#[test]
fn pipeline_creation_test(){
    init();
    let pipeline = gst::Pipeline::new(None);

    let webrtcredux = WebRtcRedux::default();

    webrtcredux.add_ice_servers(vec![
        RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            ..Default::default()
        },
    ]);

    pipeline.add_many(&[&webrtcredux]).unwrap();

    assert_eq!(pipeline.set_state(gst::State::Playing).unwrap(), gst::StateChangeSuccess::Success);
}

#[test]
fn test_sdp_serialization() {
    let target = "v=0
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
a=rtpmap:99 h263-1998/90000";

    let props = vec![
        SdpProp::Version(0), 
        SdpProp::Origin { 
            username: "jdoe".to_string(), 
            session_id: "2890844526".to_string(), 
            session_version: 2890842807, 
            net_type: NetworkType::Internet, 
            address_type: AddressType::IPv4, 
            address: "10.47.16.5".to_string()
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
            num_addresses: None
        },
        SdpProp::Timing { start: 2873397496, stop: 2873404696 },
        SdpProp::Attribute { key: "recvonly".to_string(), value: None },
        SdpProp::Media { 
            r#type: MediaType::Audio, 
            ports: vec![49170], 
            protocol: MediaProtocol::RtpAvp, 
            format: "0".to_string(), 
            props: vec![] 
        },
        SdpProp::Media { 
            r#type: MediaType::Video, 
            ports: vec![51372], 
            protocol: MediaProtocol::RtpAvp, 
            format: "99".to_string(), 
            props: vec![
                MediaProp::Attribute { key: "rtpmap".to_string(), value: Some("99 h263-1998/90000".to_string()) }
            ] 
        }
    ];

    let test = SDP { props };

    assert_eq!(test.to_string(), target);
}

#[test]
fn test_sdp_deserialization() {
    let props = vec![
        SdpProp::Version(0), 
        SdpProp::Origin { 
            username: "jdoe".to_string(), 
            session_id: "2890844526".to_string(), 
            session_version: 2890842807, 
            net_type: NetworkType::Internet, 
            address_type: AddressType::IPv4, 
            address: "10.47.16.5".to_string()
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
            num_addresses: None
        },
        SdpProp::Timing { start: 2873397496, stop: 2873404696 },
        SdpProp::Attribute { key: "recvonly".to_string(), value: None },
        SdpProp::Media { 
            r#type: MediaType::Audio, 
            ports: vec![49170], 
            protocol: MediaProtocol::RtpAvp, 
            format: "0".to_string(), 
            props: vec![] 
        },
        SdpProp::Media { 
            r#type: MediaType::Video, 
            ports: vec![51372], 
            protocol: MediaProtocol::RtpAvp, 
            format: "99".to_string(), 
            props: vec![
                MediaProp::Attribute { key: "rtpmap".to_string(), value: Some("99 h263-1998/90000".to_string()) }
            ] 
        }
    ];

    let target = SDP { props };

    let test = "v=0
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
a=rtpmap:99 h263-1998/90000";

    let res = SDP::from_str(test);

    assert!(res.is_ok(), "Parse failed with error: {:?}", res.err().unwrap());

    assert_eq!(res.unwrap(), target);
}