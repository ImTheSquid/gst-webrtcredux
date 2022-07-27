use std::str::FromStr;
use gst::{Element};
use gst::prelude::*;
use anyhow::Result;
use webrtcredux::{RTCIceConnectionState, RTCSdpType};

use webrtcredux::webrtcredux::{
    sdp::{SDP},
    RTCIceServer, WebRtcRedux,
};

pub fn must_read_stdin() -> Result<String> {
    let mut line = String::new();

    std::io::stdin().read_line(&mut line)?;
    line = line.trim().to_owned();
    println!();

    Ok(line)
}

pub fn decode(s: &str) -> Result<String> {
    let b = base64::decode(s)?;

    let s = String::from_utf8(b)?;
    Ok(s)
}

#[tokio::main]
async fn main() -> Result<()> {
    gst::init().unwrap();
    webrtcredux::plugin_register_static().unwrap();

    let pipeline = gst::Pipeline::new(None);

    let webrtcredux = WebRtcRedux::default();

    webrtcredux.add_ice_servers(vec![RTCIceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_string()],
        ..Default::default()
    }]);

    pipeline
        .add(&webrtcredux)
        .expect("Failed to add webrtcredux to the pipeline");

    let src  =gst::ElementFactory::make("videotestsrc", None)?;

    let encoder = gst::ElementFactory::make("x264enc", None)?;

    pipeline.add_many(&[&src, &encoder])?;

    Element::link_many(&[&src, &encoder, webrtcredux.as_ref()])?;

    webrtcredux.set_stream_id("video_0", "webrtc-rs")?;

    let line = must_read_stdin()?;
    let sdp_offer_from_b64 = decode(line.as_str())?;
    let offer = SDP::from_str(&sdp_offer_from_b64).expect("Failed to parse SDP");

    pipeline.set_state(gst::State::Playing)?;

    webrtcredux.on_ice_connection_state_change(Box::new(move |connection_state: RTCIceConnectionState| {
        println!("Connection State has changed {}", connection_state);
    }))
        .await?;

    webrtcredux.set_remote_description(&offer, RTCSdpType::Offer).await?;

    let answer = webrtcredux.create_answer(None).await?;

    let mut gather_complete = webrtcredux.gathering_complete_promise().await?;

    webrtcredux.set_local_description(&answer, RTCSdpType::Answer).await?;
    
    let _ = gather_complete.recv().await;

    if let Ok(Some(local_desc)) = webrtcredux.local_description().await {
        let b64 = base64::encode(local_desc.to_string());
        println!("{}", b64);
    } else {
        println!("generate local_description failed!");
    }
    
    tokio::signal::ctrl_c().await?;

    Ok(())
}
