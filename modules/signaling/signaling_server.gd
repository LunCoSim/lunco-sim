extends Node

class_name SignalingServer

# Dedicated signaling server for WebRTC (offers/answers/candidates/poll)
# Use this for all multiplayer and browser connections


# Simple in-memory signaling server for WebRTC
# Endpoints (all JSON):
# POST /signal/join     {"room":"room1","id":"peer1"}
# POST /signal/offer    {"room":"room1","from":"peer1","sdp":"..."}
# POST /signal/answer   {"room":"room1","from":"peer2","to":"peer1","sdp":"..."}
# POST /signal/candidate {"room":"room1","from":"peer1","to":"peer2","candidate":{...}}
# GET  /signal/poll?room=room1&id=peer1  => returns [ {type, from, to, payload}, ... ] and clears them for that peer

var rooms_messages := {} # { room_name: [ message_dicts ] }
var rooms_peers := {} # { room_name: [ peer_ids ] }

func _ready():
    pass

func _ensure_room(room: String) -> void:
    if room == null:
        return
    if not rooms_messages.has(room):
        rooms_messages[room] = []
    if not rooms_peers.has(room):
        rooms_peers[room] = []

func handle_request(request, response) -> void:
    var path = request.path
    var method = request.method

    # Normalize path under /signal
    if path.begins_with("/signal"):
        var sub = path.substr(7, path.length())
        # remove leading slash
        if sub.begins_with("/"):
            sub = sub.substr(1, sub.length())

        if method == "POST":
            var body = request.get_body_parsed()
            var room = str(body.get("room", ""))
            if room == "":
                response.send_json({"error": "room required"})
                return
            _ensure_room(room)

            match sub:
                "join":
                    var id = str(body.get("id", ""))
                    if id != "" and id not in rooms_peers[room]:
                        rooms_peers[room].append(id)
                    response.send_json({"success": true, "peers": rooms_peers[room]})
                    return
                "offer":
                    var from_id = str(body.get("from", ""))
                    var sdp = body.get("sdp", "")
                    rooms_messages[room].append({"type":"offer","from":from_id,"to":body.get("to",""),"payload":sdp})
                    response.send_json({"success": true})
                    return
                "answer":
                    var from_id = str(body.get("from", ""))
                    var to_id = str(body.get("to", ""))
                    var sdp = body.get("sdp", "")
                    rooms_messages[room].append({"type":"answer","from":from_id,"to":to_id,"payload":sdp})
                    response.send_json({"success": true})
                    return
                "candidate":
                    var from_id = str(body.get("from", ""))
                    var to_id = str(body.get("to", ""))
                    var candidate = body.get("candidate", null)
                    rooms_messages[room].append({"type":"candidate","from":from_id,"to":to_id,"payload":candidate})
                    response.send_json({"success": true})
                    return
                "leave":
                    var id = str(body.get("id", ""))
                    if id != "":
                        rooms_peers[room].erase(id)
                    response.send_json({"success": true})
                    return
                _:
                    response.send_error(404, "Unknown signaling POST endpoint")
                    return

        elif method == "GET":
            # Polling endpoint: /signal/poll?room=room1&id=peer1
            if sub.begins_with("poll") or sub == "poll":
                var room = request.get_parameter("room", "")
                var id = request.get_parameter("id", "")
                if room == "" or id == "":
                    response.send_json({"error":"room and id required"})
                    return
                _ensure_room(room)
                var out := []
                # collect messages addressed to this peer or broadcast (to == ""), remove them
                var remaining := []
                for msg in rooms_messages[room]:
                    var to = str(msg.get("to", ""))
                    if to == "" or to == id or msg.get("from","") == id:
                        out.append(msg)
                    else:
                        remaining.append(msg)
                rooms_messages[room] = remaining
                response.send_json({"messages": out})
                return

    response.send_error(404, "Not Found")