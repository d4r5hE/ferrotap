//! G4 gRPC decoding with .proto files.
use ferrotap::protoschema::*;

const PROTO: &str = r#"
syntax = "proto3";
package shop.v1;
import "google/protobuf/empty.proto";
option go_package = "x/y";

// A product.
message Item {
  string name = 1;
  int64 price_cents = 2 [deprecated = true];
  Kind kind = 3;
  repeated int32 tags = 4;
  map<string, int32> stock = 5;
  Dims dims = 6;
  oneof extra { string note = 7; bool gift = 8; }
  sint32 delta = 9;
  double weight = 10;
  message Dims { float w = 1; float h = 2; }
  enum Kind { KIND_UNKNOWN = 0; BOOK = 1; TOY = 2; }
  reserved 20 to 30;
}

message GetItemRequest { string id = 1; }

service Shop {
  rpc GetItem (GetItemRequest) returns (Item);
  rpc Watch (stream GetItemRequest) returns (stream Item) { option (x) = "y"; }
}
"#;

fn key(n: u64, wt: u64) -> Vec<u8> {
    varint((n << 3) | wt)
}
fn varint(mut v: u64) -> Vec<u8> {
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
fn len_field(n: u64, data: &[u8]) -> Vec<u8> {
    let mut o = key(n, 2);
    o.extend(varint(data.len() as u64));
    o.extend_from_slice(data);
    o
}
fn frame(msg: &[u8]) -> Vec<u8> {
    let mut o = vec![0u8];
    o.extend((msg.len() as u32).to_be_bytes());
    o.extend_from_slice(msg);
    o
}

#[test]
fn parses_proto_files() {
    let s = parse(PROTO).unwrap();
    let item = &s.messages["shop.v1.Item"];
    assert_eq!(item.iter().find(|f| f.name == "kind").unwrap().ty, "shop.v1.Item.Kind");
    assert_eq!(item.iter().find(|f| f.name == "dims").unwrap().ty, "shop.v1.Item.Dims");
    assert!(item.iter().find(|f| f.name == "tags").unwrap().repeated);
    assert_eq!(item.iter().find(|f| f.name == "stock").unwrap().map, Some(("string".into(), "int32".into())));
    assert!(item.iter().any(|f| f.name == "gift" && f.number == 8), "oneof members are fields");
    assert_eq!(s.enums["shop.v1.Item.Kind"][&2], "TOY");
    assert_eq!(s.rpcs["/shop.v1.Shop/GetItem"], ("shop.v1.GetItemRequest".to_string(), "shop.v1.Item".to_string()));
    assert!(s.rpcs.contains_key("/shop.v1.Shop/Watch"));
    assert!(parse("message { }").is_err());
}

#[test]
fn decodes_grpc_with_names() {
    let s = parse(PROTO).unwrap();
    let mut item = len_field(1, b"Rust book");
    item.extend(key(2, 0));
    item.extend(varint(4999));
    item.extend(key(3, 0));
    item.extend(varint(1));
    item.extend(len_field(4, &[1, 2, 3])); // packed
    let mut entry = len_field(1, b"berlin");
    entry.extend(key(2, 0));
    entry.extend(varint(7));
    item.extend(len_field(5, &entry));
    let mut dims = key(1, 5);
    dims.extend(2.5f32.to_le_bytes());
    item.extend(len_field(6, &dims));
    item.extend(key(8, 0));
    item.extend(varint(1));
    item.extend(key(9, 0));
    item.extend(varint(3)); // zigzag -2
    item.extend(key(10, 1));
    item.extend(1.25f64.to_le_bytes());
    item.extend(key(99, 0));
    item.extend(varint(5)); // unknown field
    let t = grpc_view_typed(&s, "/shop.v1.Shop/GetItem", &frame(&item), true).unwrap();
    for want in ["(shop.v1.Item)", "name: \"Rust book\"", "price_cents: 4999", "kind: BOOK", "tags: [1, 2, 3]", "stock[\"berlin\"]: 7", "dims {", "w: 2.5", "gift: true", "delta: -2", "weight: 1.25", "99: 5"] {
        assert!(t.contains(want), "missing {want} in\n{t}");
    }
    let req = grpc_view_typed(&s, "https://h/shop.v1.Shop/GetItem", &frame(&len_field(1, b"42")), false).unwrap();
    assert!(req.contains("id: \"42\""), "{req}");
    assert!(grpc_view_typed(&s, "/other.Svc/X", &frame(&[]), false).is_err());
}

#[test]
fn loads_files_and_folders() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.proto"), PROTO).unwrap();
    std::fs::write(d.path().join("b.proto"), "syntax = \"proto3\"; package other; message Ping { int32 n = 1; }").unwrap();
    std::fs::write(d.path().join("readme.txt"), "not proto").unwrap();
    let s = load_files(&[d.path().to_path_buf()]).unwrap();
    assert!(s.messages.contains_key("shop.v1.Item") && s.messages.contains_key("other.Ping"));
    let cfg = d.path().join("protos.json");
    save_paths(&cfg, &["x.proto".to_string()]).unwrap();
    assert_eq!(load_paths(&cfg), vec!["x.proto"]);
    assert!(load_files(&[d.path().join("missing.proto")]).is_err());
}
