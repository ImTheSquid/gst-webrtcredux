use std::str::FromStr;
use gst::{Element};
use gst::prelude::*;
use anyhow::Result;
use clipboard::{ClipboardContext, ClipboardProvider};
use tokio::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use webrtcredux::{RTCIceConnectionState, RTCSdpType};
use tokio::runtime::Handle;
use webrtcredux::sdp::LineEnding::LF;

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

    let video_src = gst::ElementFactory::make("videotestsrc").build()?;

    let video_encoder = gst::ElementFactory::make("x264enc").build()?;

    video_encoder.set_property("threads", 12u32);
    video_encoder.set_property("bitrate", 2048000_u32 / 1000);
    video_encoder.set_property_from_str("tune", "zerolatency");
    video_encoder.set_property_from_str("speed-preset", "ultrafast");
    video_encoder.set_property("key-int-max", 2560u32);
    video_encoder.set_property("b-adapt", false);
    video_encoder.set_property("vbv-buf-capacity", 120u32);

    pipeline.add_many(&[&video_src, &video_encoder])?;

    Element::link_many(&[&video_src, &video_encoder])?;

    video_encoder.link(webrtcredux.upcast_ref::<gst::Element>())?;

    //webrtcredux.set_stream_id("video_0", "webrtc-rs")?;

    let audio_src = gst::ElementFactory::make("audiotestsrc").build()?;

    audio_src.set_property_from_str("wave", "ticks");
    audio_src.set_property_from_str("tick-interval", "500000000");

    let audio_encoder = gst::ElementFactory::make("opusenc").build()?;

    let audio_capsfilter = gst::ElementFactory::make("capsfilter").build()?;

    //Create a vector containing the option of the gst caps
    let caps_options: Vec<(&str, &(dyn ToSendValue + Sync))> =
        vec![("channels", &2)];

    audio_capsfilter.set_property(
        "caps",
        &gst::Caps::new_simple("audio/x-raw", caps_options.as_ref()),
    );

    pipeline.add_many(&[&audio_src, &audio_capsfilter, &audio_encoder])?;

    Element::link_many(&[&audio_src, &audio_capsfilter, &audio_encoder])?;

    audio_encoder.link(webrtcredux.upcast_ref::<gst::Element>())?;

    //webrtcredux.set_stream_id("audio_0", "webrtc-rs")?;

    let mut clipboard_handle = ClipboardContext::new().expect("Failed to create clipboard context");

    print!("Please copy remote description to your clipboard");
    pause().await;

    let line = clipboard_handle.get_contents().expect("Failed to get clipboard contents");
    let sdp_offer_from_b64 = decode(line.as_str())?;
    let offer = SDP::from_str(&sdp_offer_from_b64).expect("Failed to parse SDP");

    pipeline.set_state(gst::State::Playing)?;

    webrtcredux.on_peer_connection_state_change(Box::new(|state| {
        println!("Peer connection state has changed {}", state);

        Box::pin(async {})
    }))?;

    webrtcredux.on_ice_connection_state_change(Box::new(move |connection_state: RTCIceConnectionState| {
        println!("Connection State has changed {}", connection_state);

        Box::pin(async {})
    }))
        .await?;

    webrtcredux.set_remote_description(&offer, RTCSdpType::Offer).await?;

    let answer = webrtcredux.create_answer(None).await?;

    let mut gather_complete = webrtcredux.gathering_complete_promise().await?;

    webrtcredux.set_local_description(&answer, RTCSdpType::Answer).await?;
    
    let _ = gather_complete.recv().await;

    if let Ok(Some(local_desc)) = webrtcredux.local_description().await {
        let b64 = base64::encode(local_desc.to_string(LF));
        clipboard_handle.set_contents(b64.clone()).expect("Failed to set clipboard contents");
        println!("Base64 Session Description for the browser copied to the clipboard", );
        println!("{}", b64);
    } else {
        println!("generate local_description failed!");
    }
    
    tokio::signal::ctrl_c().await?;

    Ok(())
}
