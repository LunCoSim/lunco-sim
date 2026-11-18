//! Native, operator-configured HTTP connections for presentation programs.
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, LazyLock, OnceLock},
    time::{Duration, Instant},
};

use bevy::prelude::Resource;
use lunco_scripting_bridge_core as bridge;
use lunco_workspace::{TwinId, WorkspaceResource};
use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Map};
use serde::Deserialize;

const BODY_LIMIT: u64 = 60 * 1024;
static CLIENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into()
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Connection {
    twin: PathBuf,
    url: String,
    method: String,
    bearer_env: Option<String>,
}

struct Response {
    status: u16,
    text: String,
}
type Reply = Result<Response, String>;
struct Request {
    twin: u64,
    started: Instant,
    reply: Arc<OnceLock<Reply>>,
}
#[derive(Resource, Default)]
struct Requests {
    next: i64,
    pending: HashMap<i64, Request>,
}

fn target() -> Result<u64, String> {
    bridge::ensure_script_effect_allowed()?;
    let ctx = bridge::execution_context();
    if ctx.clock == lunco_core::RuntimeClock::Simulation {
        return Err("HTTP requires a presentation or operator callback".into());
    }
    ctx.target_twin()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "HTTP requires a Twin-scoped invocation".into())
}

fn transfer(c: Connection, body: Option<String>) -> Reply {
    let token = c
        .bearer_env
        .as_ref()
        .map(|key| std::env::var(key).map_err(|_| "HTTP credential unavailable"))
        .transpose()?;
    let auth = token.map(|v| format!("Bearer {v}"));
    let mut response = match (c.method.as_str(), body) {
        ("GET", None) => {
            let mut req = CLIENT.get(c.url.as_str());
            if let Some(auth) = auth {
                req = req.header("Authorization", auth);
            }
            req.header("Accept", "application/json").call()
        }
        ("POST", Some(data)) => {
            let mut req = CLIENT.post(c.url.as_str());
            if let Some(auth) = auth {
                req = req.header("Authorization", auth);
            }
            req.header("Accept", "application/json")
                .content_type("application/json")
                .send(data.as_bytes())
        }
        _ => return Err("GET takes unit; POST takes a JSON value".into()),
    }
    .map_err(|error| match error {
        ureq::Error::Timeout(_) => "HTTP request timed out",
        ureq::Error::HostNotFound => "HTTP host could not be resolved",
        ureq::Error::ConnectionFailed => "HTTP connection failed",
        ureq::Error::Rustls(_) => "HTTPS certificate or TLS negotiation failed",
        _ => "HTTP transport failed",
    })?;
    let status = response.status().as_u16();
    let bytes = response
        .body_mut()
        .with_config()
        .limit(BODY_LIMIT)
        .read_to_vec()
        .map_err(|_| "Response exceeded 60 KiB or could not be read")?;
    Ok(Response {
        status,
        text: String::from_utf8(bytes).map_err(|_| "Expected a UTF-8 response")?,
    })
}

fn start(name: ImmutableString, data: Dynamic) -> Result<i64, Box<EvalAltResult>> {
    let twin = target()?;
    let connections = std::env::var("LUNCOSIM_HTTP_CONNECTIONS")
        .map_err(|_| "Configure LUNCOSIM_HTTP_CONNECTIONS for this Twin")?;
    let mut connections: HashMap<String, Connection> =
        serde_json::from_str(&connections).map_err(|_| "Invalid HTTP connection configuration")?;
    let c = connections
        .remove(name.as_str())
        .ok_or("Unknown HTTP connection")?;
    let url = url::Url::parse(&c.url).map_err(|_| "Invalid HTTP endpoint URL")?;
    let encrypted = url.scheme() == "https";
    let local = matches!(url.host(),Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        || matches!(url.host(),Some(url::Host::Ipv6(ip)) if ip.is_loopback());
    if !(encrypted || (local && url.scheme() == "http" && c.bearer_env.is_none()))
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Use HTTPS, or credential-free HTTP to a loopback address".into());
    }
    let body = if data.is_unit() {
        None
    } else {
        let json = rhai::serde::from_dynamic::<serde_json::Value>(&data)?.to_string();
        if json.len() as u64 > BODY_LIMIT {
            return Err("HTTP request exceeds 60 KiB".into());
        }
        Some(json)
    };
    bridge::with_world(|w| -> Result<i64, String> {
        let ws = w
            .get_resource::<WorkspaceResource>()
            .ok_or("No active workspace")?;
        let record = ws.0.twin(TwinId::new(twin)).ok_or("HTTP Twin is closed")?;
        if record.root != c.twin {
            return Err("HTTP connection belongs to a different Twin".into());
        }
        let mut requests = w.get_resource_or_init::<Requests>();
        requests.pending.retain(|_, r| {
            r.started.elapsed() < Duration::from_secs(120) || r.reply.get().is_none()
        });
        if requests.pending.len() >= 32 {
            return Err("Too many outstanding HTTP requests".into());
        }
        if requests
            .pending
            .values()
            .filter(|r| r.reply.get().is_none())
            .count()
            >= 4
        {
            return Err("HTTP workers are busy".into());
        }
        let reply = Arc::new(OnceLock::new());
        let worker = Arc::clone(&reply);
        std::thread::Builder::new()
            .name("lunco-http".into())
            .spawn(move || {
                let _ = worker.set(transfer(c, body));
            })
            .map_err(|_| "Could not start HTTP worker")?;
        requests.next += 1;
        let id = requests.next;
        requests.pending.insert(
            id,
            Request {
                twin,
                started: Instant::now(),
                reply,
            },
        );
        Ok(id)
    })
    .ok_or("No active scripting world")?
    .map_err(Into::into)
}

fn poll(id: i64) -> Result<Dynamic, Box<EvalAltResult>> {
    let twin = target()?;
    bridge::with_world(|w| -> Result<Dynamic, String> {
        let mut requests = w
            .get_resource_mut::<Requests>()
            .ok_or("Unknown HTTP request")?;
        let request = requests
            .pending
            .get(&id)
            .filter(|r| r.twin == twin)
            .ok_or("Unknown or expired HTTP request")?;
        let Some(reply) = request.reply.get() else {
            return Ok(Dynamic::UNIT);
        };
        let mut result = Map::new();
        match reply {
            Ok(r) => {
                result.insert("ok".into(), (200..300).contains(&r.status).into());
                result.insert("status".into(), i64::from(r.status).into());
                result.insert("text".into(), r.text.clone().into());
                result.insert("error".into(), Dynamic::UNIT);
            }
            Err(e) => {
                result.insert("ok".into(), false.into());
                result.insert("status".into(), Dynamic::UNIT);
                result.insert("text".into(), Dynamic::UNIT);
                result.insert("error".into(), e.clone().into());
            }
        }
        requests.pending.remove(&id);
        Ok(result.into())
    })
    .ok_or("No active scripting world")?
    .map_err(Into::into)
}

pub(super) fn register(engine: &mut Engine) {
    engine.register_fn("http_request", start);
    engine.register_fn("http_result", poll);
}
