//! G4 gRPC server reflection against a fake HTTP/2 (h2c) reflection server.
use std::convert::Infallible;

use ferrotap::grpc_reflect::*;
use http_body_util::{BodyExt, Full};
use hudsucker::hyper::{self, body::Bytes, Request, Response};
use hudsucker::hyper_util::rt::{TokioExecutor, TokioIo};

fn file_descriptor() -> Vec<u8> {
    let field = |name: &str, num: u64, ty: u64, type_name: Option<&str>| {
        let mut f = pb_bytes(1, name.as_bytes());
        f.extend(pb_varint(3, num));
        f.extend(pb_varint(4, 1));
        f.extend(pb_varint(5, ty));
        if let Some(t) = type_name {
            f.extend(pb_bytes(6, t.as_bytes()));
        }
        f
    };
    let mut kind = pb_bytes(1, b"Kind");
    let mut v = pb_bytes(1, b"BOOK");
    v.extend(pb_varint(2, 1));
    kind.extend(pb_bytes(2, &v));
    let mut item = pb_bytes(1, b"Item");
    item.extend(pb_bytes(2, &field("name", 1, 9, None)));
    item.extend(pb_bytes(2, &field("kind", 2, 14, Some(".shop.v1.Item.Kind"))));
    item.extend(pb_bytes(4, &kind));
    let mut req = pb_bytes(1, b"Req");
    req.extend(pb_bytes(2, &field("id", 1, 9, None)));
    let mut method = pb_bytes(1, b"Get");
    method.extend(pb_bytes(2, b".shop.v1.Req"));
    method.extend(pb_bytes(3, b".shop.v1.Item"));
    let mut svc = pb_bytes(1, b"Shop");
    svc.extend(pb_bytes(2, &method));
    let mut fd = pb_bytes(1, b"shop.proto");
    fd.extend(pb_bytes(2, b"shop.v1"));
    fd.extend(pb_bytes(4, &item));
    fd.extend(pb_bytes(4, &req));
    fd.extend(pb_bytes(6, &svc));
    fd
}

fn frame(m: &[u8]) -> Vec<u8> {
    let mut o = vec![0u8];
    o.extend((m.len() as u32).to_be_bytes());
    o.extend_from_slice(m);
    o
}

async fn handle(req: Request<hyper::body::Incoming>) -> Result<Response<http_body_util::combinators::BoxBody<Bytes, Infallible>>, Infallible> {
    let path = req.uri().path().to_string();
    let body = req.into_body().collect().await.map(|c| c.to_bytes()).unwrap_or_default();
    let mut trailers = hyper::HeaderMap::new();
    if !path.starts_with("/grpc.reflection.v1alpha.") {
        trailers.insert("grpc-status", "12".parse().unwrap()); // unimplemented
        let b = Full::new(Bytes::new()).with_trailers(async move { Some(Ok(trailers)) }).boxed();
        return Ok(Response::builder().header("content-type", "application/grpc").body(b).unwrap());
    }
    let msg = &body[5..];
    let answer = if msg.first() == Some(&((7 << 3) | 2)) {
        let mut svc = pb_bytes(1, b"shop.v1.Shop");
        let refl = pb_bytes(1, b"grpc.reflection.v1alpha.ServerReflection");
        let mut list = pb_bytes(1, &svc);
        list.extend(pb_bytes(1, &refl));
        svc.clear();
        pb_bytes(6, &list)
    } else {
        pb_bytes(4, &pb_bytes(1, &file_descriptor()))
    };
    trailers.insert("grpc-status", "0".parse().unwrap());
    let b = Full::new(Bytes::from(frame(&answer))).with_trailers(async move { Some(Ok(trailers)) }).boxed();
    Ok(Response::builder().header("content-type", "application/grpc").body(b).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reflection_builds_a_schema() {
    // descriptor parsing on its own
    let mut s = ferrotap::protoschema::Schema::default();
    add_file_descriptor(&mut s, &file_descriptor());
    assert_eq!(s.rpcs["/shop.v1.Shop/Get"], ("shop.v1.Req".to_string(), "shop.v1.Item".to_string()));
    assert_eq!(s.enums["shop.v1.Item.Kind"][&1], "BOOK");

    // over the wire
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new()).serve_connection(TokioIo::new(stream), hyper::service::service_fn(handle)).await;
            });
        }
    });
    let (schema, services) = reflect(&format!("http://127.0.0.1:{port}")).await.unwrap();
    assert_eq!(services, vec!["shop.v1.Shop", "grpc.reflection.v1alpha.ServerReflection"]);
    assert!(schema.messages.contains_key("shop.v1.Item") && schema.messages.contains_key("shop.v1.Req"));
    // the schema decodes a response
    let mut item = pb_bytes(1, b"Rust");
    item.extend(pb_varint(2, 1));
    let t = ferrotap::protoschema::grpc_view_typed(&schema, "/shop.v1.Shop/Get", &frame(&item), true).unwrap();
    assert!(t.contains("name: \"Rust\"") && t.contains("kind: BOOK"), "{t}");
    assert!(reflect("http://127.0.0.1:1").await.is_err());
}
