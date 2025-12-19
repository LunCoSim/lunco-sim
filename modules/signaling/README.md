Signaling router for WebRTC

This project includes a very small HTTP-based signaling router at `/signal` implemented by `SignalingRouter`.

Purpose
- Provide a simple way for WebRTC peers (clients) to exchange SDP offers/answers and ICE candidates via the existing `HttpServer`.
- Keep the project compatible with WebSocket-based multiplayer (no removal).

Endpoints
- POST /signal/join
  - JSON body: {"room":"room1","id":"peer1"}
  - Response: {"success":true, "peers":[...]} - lists peers already in the room

- POST /signal/offer
  - JSON body: {"room":"room1","from":"peer1","to":"","sdp":"..."}
  - Stores an "offer" message for the room ("to" may be empty for broadcast)

- POST /signal/answer
  - JSON body: {"room":"room1","from":"peer2","to":"peer1","sdp":"..."}
  - Stores an "answer" message targeted to `to`

- POST /signal/candidate
  - JSON body: {"room":"room1","from":"peer1","to":"peer2","candidate":{...}}
  - Stores an ICE candidate message targeted to `to`

- POST /signal/leave
  - JSON body: {"room":"room1","id":"peer1"}
  - Peer leaves the room

- GET /signal/poll?room=room1&id=peer1
  - Returns JSON: {"messages": [ {type, from, to, payload}, ... ] }
  - The server clears messages it returns for that peer (basic polling semantics)

How to wire with Godot (client-side)
- Keep using `WebSocketMultiplayerPeer` for WebSocket transport as before.
- To use WebRTC: create a `WebRTCMultiplayerPeer` and assign it to `multiplayer.multiplayer_peer`.
- Use the signaling endpoints above to exchange SDP and ICE between peers.

Example (high level):
1. Client A (caller) posts /signal/join with its id and room.
2. Client B (callee) joins as well.
3. Client A creates an offer via `webrtc_peer.create_local_offer()` (or the equivalent API in your Godot version) and posts it to /signal/offer with from=A, to=B.
4. Client B polls /signal/poll and receives the offer. B sets the remote SDP and creates an answer; B POSTs /signal/answer.
5. A polls /signal/poll, receives the answer and sets remote SDP.
6. Both sides exchange ICE candidates via /signal/candidate.

Notes and next steps
- This signaling router is minimal and uses in-memory storage. For production use, persist messages and add auth, TTLs and cleaning.
- It uses polling. If you want real-time signaling, implement WebSocket-based signaling or SSE.
- `networking.gd` was left mostly unchanged to preserve WebSocket behavior. You can add WebRTC glue in `core/singletones/networking.gd` to call these endpoints and feed `WebRTCMultiplayerPeer`.

If you want, I can now:
- Add helper functions in `core/singletones/networking.gd` to perform the HTTP signaling flows for client/server WebRTC setup.
- Replace polling with WebSocket signaling server instead.
