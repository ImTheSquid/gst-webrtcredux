use gst::prelude::*;

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

    let webrtcredux = webrtcredux::webrtcredux::WebRtcRedux::default();

    webrtcredux.set_property("ice-servers", &vec!["stun:stun.l.google.com:19302".to_string()]);

    pipeline.add_many(&[&webrtcredux]).unwrap();

    assert_eq!(pipeline.set_state(gst::State::Playing).unwrap(), gst::StateChangeSuccess::Success);
}