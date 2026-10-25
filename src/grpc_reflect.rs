//! G4 gRPC server reflection: ask a server for its services and their file descriptors
//! (grpc.reflection.v1alpha, falling back to v1) and turn the FileDescriptorProtos into the same
//! schema the .proto reader builds, so calls decode with field names without any .proto file.
use http_body_util::{BodyExt, Full};
use hudsucker::hyper::body::Bytes;
use hudsucker::hyper_util::{client::legacy::Client, rt::TokioExecutor};

use crate::protoschema::{Field, Schema};

// ------------------------------------------------------------------------------------------ protobuf helpers

fn varint(v: u64) -> Vec<u8> {
    let mut v = v;
    let mut o = Vec::new();
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            o.push(b);
            return o;
        }
        o.push(b | 0x80);
    }
}

/// A length-delimited field.
pub fn pb_bytes(field: u64, data: &[u8]) -> Vec<u8> {
    let mut o = varint((field << 3) | 2);
    o.extend(varint(data.len() as u64));
    o.extend_from_slice(data);
    o
}

pub fn pb_varint(field: u64, v: u64) -> Vec<u8> {
    let mut o = varint(field << 3);
    o.extend(varint(v));
    o
}

fn read_varint(b: &[u8], p: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for i in 0..10 {
        let x = *b.get(*p)?;
        *p += 1;
        v |= ((x & 0x7f) as u64) << (7 * i);
        if x & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// Top-level fields of a message: (number, varint value or bytes).
pub fn fields(b: &[u8]) -> Vec<(u64, Result<u64, Vec<u8>>)> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < b.len() {
        let Some(key) = read_varint(b, &mut p) else { break };
        match key & 7 {
            0 => match read_varint(b, &mut p) {
                Some(v) => out.push((key >> 3, Ok(v))),
                None => break,
            },
            2 => {
                let Some(n) = read_varint(b, &mut p) else { break };
                let Some(s) = b.get(p..p + n as usize) else { break };
                out.push((key >> 3, Err(s.to_vec())));
                p += n as usize;
            }
            1 => p += 8,
            5 => p += 4,
            _ => break,
        }
    }
    out
}

fn text(v: &Result<u64, Vec<u8>>) -> String {
    match v {
        Err(b) => String::from_utf8_lossy(b).into_owned(),
        Ok(_) => String::new(),
    }
}

// ------------------------------------------------------------------------------------------ descriptors

fn scalar(t: u64) -> &'static str {
    match t {
        1 => "double",
        2 => "float",
        3 => "int64",
        4 => "uint64",
        5 => "int32",
        6 => "fixed64",
        7 => "fixed32",
        8 => "bool",
        9 => "string",
        12 => "bytes",
        13 => "uint32",
        15 => "sfixed32",
        16 => "sfixed64",
        17 => "sint32",
        18 => "sint64",
        _ => "",
    }
}

fn add_enum(schema: &mut Schema, full: &str, b: &[u8]) {
    let mut vals = std::collections::HashMap::new();
    for (n, v) in fields(b) {
        if n == 2 {
            if let Err(ev) = v {
                let f = fields(&ev);
                let name = f.iter().find(|x| x.0 == 1).map(|x| text(&x.1)).unwrap_or_default();
                let num = f.iter().find(|x| x.0 == 2).and_then(|x| x.1.as_ref().ok().copied()).unwrap_or(0);
                vals.insert(num as i64 as i32 as i64, name);
            }
        }
    }
    schema.enums.insert(full.to_string(), vals);
}

fn add_message(schema: &mut Schema, prefix: &str, b: &[u8]) {
    let fs = fields(b);
    let name = fs.iter().find(|x| x.0 == 1).map(|x| text(&x.1)).unwrap_or_default();
    let full = if prefix.is_empty() { name } else { format!("{prefix}.{name}") };
    let mut out = Vec::new();
    for (n, v) in &fs {
        let Err(sub) = v else { continue };
        match n {
            2 => {
                let f = fields(sub);
                let get_s = |k: u64| f.iter().find(|x| x.0 == k).map(|x| text(&x.1)).unwrap_or_default();
                let get_n = |k: u64| f.iter().find(|x| x.0 == k).and_then(|x| x.1.as_ref().ok().copied()).unwrap_or(0);
                let t = get_n(5);
                let ty = if t == 11 || t == 14 { get_s(6).trim_start_matches('.').to_string() } else { scalar(t).to_string() };
                out.push(Field { name: get_s(1), number: get_n(3), ty, repeated: get_n(4) == 3, map: None });
            }
            3 => add_message(schema, &full, sub),
            4 => {
                let en = fields(sub).iter().find(|x| x.0 == 1).map(|x| text(&x.1)).unwrap_or_default();
                add_enum(schema, &format!("{full}.{en}"), sub);
            }
            _ => {}
        }
    }
    schema.messages.insert(full, out);
}

/// Add one serialized FileDescriptorProto to `schema`.
pub fn add_file_descriptor(schema: &mut Schema, b: &[u8]) {
    let fs = fields(b);
    let package = fs.iter().find(|x| x.0 == 2).map(|x| text(&x.1)).unwrap_or_default();
    for (n, v) in &fs {
        let Err(sub) = v else { continue };
        match n {
            4 => add_message(schema, &package, sub),
            5 => {
                let en = fields(sub).iter().find(|x| x.0 == 1).map(|x| text(&x.1)).unwrap_or_default();
                add_enum(schema, &if package.is_empty() { en.clone() } else { format!("{package}.{en}") }, sub);
            }
            6 => {
                let sf = fields(sub);
                let svc = sf.iter().find(|x| x.0 == 1).map(|x| text(&x.1)).unwrap_or_default();
                let svc_full = if package.is_empty() { svc } else { format!("{package}.{svc}") };
                for (mn, mv) in &sf {
                    if *mn != 2 {
                        continue;
                    }
                    let Err(m) = mv else { continue };
                    let mf = fields(m);
                    let g = |k: u64| mf.iter().find(|x| x.0 == k).map(|x| text(&x.1)).unwrap_or_default();
                    schema.rpcs.insert(format!("/{svc_full}/{}", g(1)), (g(2).trim_start_matches('.').to_string(), g(3).trim_start_matches('.').to_string()));
                }
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------------------------------------ the RPC

/// One request / one response on the reflection stream; returns the response message bytes.
async fn reflect_call(client: &Client<impl hudsucker::hyper_util::client::legacy::connect::Connect + Clone + Send + Sync + 'static, Full<Bytes>>, base: &str, path: &str, msg: &[u8]) -> Result<Vec<u8>, String> {
    let mut framed = vec![0u8];
    framed.extend((msg.len() as u32).to_be_bytes());
    framed.extend_from_slice(msg);
    let req = hudsucker::hyper::Request::post(format!("{}{path}", base.trim_end_matches('/')))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .body(Full::new(Bytes::from(framed)))
        .map_err(|e| e.to_string())?;
    let resp = tokio::time::timeout(std::time::Duration::from_secs(15), client.request(req)).await.map_err(|_| "the server did not answer".to_string())?.map_err(|e| crate::proxy::error_chain(&e))?;
    let status = resp.status();
    let collected = resp.into_body().collect().await.map_err(|e| e.to_string())?;
    if let Some(t) = collected.trailers() {
        if let Some(s) = t.get("grpc-status").and_then(|v| v.to_str().ok()) {
            if s != "0" {
                let m = t.get("grpc-message").and_then(|v| v.to_str().ok()).unwrap_or("");
                return Err(format!("grpc-status {s} {m}"));
            }
        }
    }
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let body = collected.to_bytes();
    let frames = crate::streams_decode::grpc_frames(&body)?;
    frames.into_iter().next().map(|f| f.data).ok_or_else(|| "empty reflection answer".to_string())
}

/// Ask the server at `base` (http://host:port for plaintext HTTP/2, or https://) for every
/// service's descriptors. Returns the schema and the service names.
pub async fn reflect(base: &str) -> Result<(Schema, Vec<String>), String> {
    let client = Client::builder(TokioExecutor::new()).http2_only(true).build(crate::timing::build_connector());
    let mut last_err = String::new();
    for path in ["/grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo", "/grpc.reflection.v1.ServerReflection/ServerReflectionInfo"] {
        // list_services (field 7)
        let resp = match reflect_call(&client, base, path, &pb_bytes(7, b"")).await {
            Ok(r) => r,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        let mut services = Vec::new();
        let mut server_err = None;
        for (n, v) in fields(&resp) {
            match (n, &v) {
                (6, Err(list)) => {
                    for (k, s) in fields(list) {
                        if let (1, Err(svc)) = (k, &s) {
                            for (kk, nm) in fields(svc) {
                                if kk == 1 {
                                    services.push(text(&nm));
                                }
                            }
                        }
                    }
                }
                (7, Err(e)) => {
                    server_err = Some(format!("reflection error: {}", fields(e).iter().find(|x| x.0 == 2).map(|x| text(&x.1)).unwrap_or_default()));
                }
                _ => {}
            }
        }
        if services.is_empty() {
            if let Some(e) = server_err {
                last_err = e;
                continue;
            }
        }
        let mut schema = Schema::default();
        for svc in services.iter().filter(|s| !s.starts_with("grpc.reflection.")) {
            // file_containing_symbol (field 4) -> file_descriptor_response (field 4) { repeated bytes = 1 }
            let r = reflect_call(&client, base, path, &pb_bytes(4, svc.as_bytes())).await?;
            for (n, v) in fields(&r) {
                if n == 4 {
                    if let Err(fdr) = v {
                        for (k, fd) in fields(&fdr) {
                            if k == 1 {
                                if let Err(fd) = fd {
                                    add_file_descriptor(&mut schema, &fd);
                                }
                            }
                        }
                    }
                }
            }
        }
        return Ok((schema, services));
    }
    Err(format!("reflection failed: {last_err}"))
}

/// `reflect` from non-async code (a private runtime).
pub fn reflect_blocking(base: &str) -> Result<(Schema, Vec<String>), String> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(reflect(base))
}
