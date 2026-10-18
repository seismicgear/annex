# Voice publication state

Camera, screen and microphone publishing slots remain negotiated for the lifetime
of a call. Stopping capture with `replaceTrack(null)` does not reliably mute or end
the remote browser's reserved receiver. RTP inactivity therefore does not determine
whether a participant intends to publish a camera or screen.

After voice admission, the server sends `webrtc_media_state` frames over the
participant's authenticated signaling socket. Each frame has `channelId`, `peerId`,
`generation`, `revision`, `present`, and `state`. State is either null (a client that
has not reported intent) or `{ microphone, camera, screenShare }` with boolean values.
The server supplies a snapshot of every current peer to a new caller and sends
subsequent changes only to callers admitted to that room.

The client takes its generation from the frame naming its own identity, then sends
`{ type: "webrtc_media_state", channelId, generation, state }`. The server derives
the sender from socket authentication, checks current membership and voice policy,
and accepts updates only for that sender's current admitted connection. A removed
member or obsolete connection cannot change a replacement connection's state.

Capture success reports an enabled source after its sender is committed. Explicit
stop and native capture loss report it disabled immediately. Receivers hide the
stopped source without destroying its reserved track, allowing the same connection
to resume. Generation and revision order prevent delayed snapshots, departures or
updates from overwriting newer state. Departure sends `present: false`.

Older clients retain their previous RTP-based display behavior until they report
intent. If the server's publication event subscriber overruns its bounded queue,
it closes that signaling session rather than silently losing a stop; re-admission
supplies a fresh snapshot. These frames use the same direct host connection as
other call signaling and never route through the invitation service.
