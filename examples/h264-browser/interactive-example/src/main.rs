use std::str::FromStr;
use gst::{Element};
use gst::prelude::*;
use anyhow::Result;
use clipboard::{ClipboardContext, ClipboardProvider};
use tokio::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use webrtcredux::{RTCIceConnectionState, RTCSdpType};
use tokio::runtime::Handle;

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

async fn pause() {
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();

    // We want the cursor to stay at the end of the line, so we print without a newline and flush manually.
    stdout.write_all(b"\nPress Enter to continue...").await.unwrap();
    stdout.flush().await.unwrap();

    // Read a single byte and discard
    let _ = stdin.read(&mut [0u8]).await.unwrap();
}

#[tokio::main]
async fn main() -> Result<()> {
    gst::init().unwrap();
    webrtcredux::plugin_register_static().unwrap();

    let pipeline = gst::Pipeline::new(None);

    let webrtcredux = WebRtcRedux::default();

    webrtcredux.set_tokio_runtime(Handle::current());

    webrtcredux.add_ice_servers(vec![RTCIceServer {
        urls: vec!["stun:stun.comrex.com:3478".to_string()],
        ..Default::default()
    }]);

    pipeline
        .add(webrtcredux.upcast_ref::<gst::Element>())
        .expect("Failed to add webrtcredux to the pipeline");

    let video_src = gst::ElementFactory::make("videotestsrc", None)?;

    let video_encoder = gst::ElementFactory::make("x264enc", None)?;

    pipeline.add_many(&[&video_src, &video_encoder])?;

    Element::link_many(&[&video_src, &video_encoder])?;

    video_encoder.link(webrtcredux.upcast_ref::<gst::Element>())?;

    //webrtcredux.set_stream_id("video_0", "webrtc-rs")?;

    let audio_src = gst::ElementFactory::make("audiotestsrc", None)?;

    audio_src.set_property_from_str("wave", "ticks");
    audio_src.set_property_from_str("tick-interval", "500000000");

    let audio_encoder = gst::ElementFactory::make("opusenc", None)?;

    pipeline.add_many(&[&audio_src, &audio_encoder])?;

    Element::link_many(&[&audio_src, &audio_encoder])?;

    audio_encoder.link(webrtcredux.upcast_ref::<gst::Element>())?;

    //webrtcredux.set_stream_id("audio_0", "webrtc-rs")?;

    let mut clipboard_handle = ClipboardContext::new().expect("Failed to create clipboard context");

    pause().await;

    let line = clipboard_handle.get_contents().expect("Failed to get clipboard contents");
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
        clipboard_handle.set_contents(b64.clone()).expect("Failed to set clipboard contents");
        println!("Base64 Session Description for the browser copied to the cliboard", );
        println!("{}", b64);
    } else {
        println!("generate local_description failed!");
    }
    
    tokio::signal::ctrl_c().await?;

    Ok(())
}
