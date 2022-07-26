let pc = new RTCPeerConnection({
    iceServers: [
        {
            urls: 'stun:stun.comrex.com:3478'
        }
    ]
})
let log = msg => {
    document.getElementById('div').innerHTML += msg + '<br>'
}

pc.ontrack = function (event) {
    var el = document.createElement(event.track.kind)
    el.srcObject = event.streams[0]
    el.autoplay = true
    el.controls = true

    document.getElementById('remoteVideos').appendChild(el)
}

pc.oniceconnectionstatechange = e => log(pc.iceConnectionState)
pc.onicecandidate = event => {
    if (event.candidate === null) {
        document.getElementById('localSessionDescription').value = btoa(pc.localDescription.sdp)
    }
}

pc.addTransceiver('video', {'direction': 'sendrecv'})
pc.addTransceiver('audio', {'direction': 'sendrecv'})

pc.createOffer().then(d => pc.setLocalDescription(d)).catch(log)

window.startSession = () => {
    let sd = document.getElementById('remoteSessionDescription').value
    if (sd === '') {
        return alert('Session Description must not be empty')
    }

    try {
        pc.setRemoteDescription(new RTCSessionDescription({type: 'answer', sdp: atob(sd)}))
    } catch (e) {
        alert(e)
    }
}
