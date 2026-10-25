//! G4 gRPC with .proto files: a small .proto (proto2 / proto3) reader for messages, nested types,
//! enums, oneof, map fields, packages and services, and a schema-aware decoder that shows field
//! names, enum names and typed values. Services map `/package.Service/Method` to its message types.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub number: u64,
    /// Scalar name ("int32", "string", ...) or the resolved full message / enum name.
    pub ty: String,
    pub repeated: bool,
    /// map<K, V>: (key type, value type).
    pub map: Option<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Schema {
    /// Full name (package.Outer.Inner) -> fields.
    pub messages: HashMap<String, Vec<Field>>,
    /// Full name -> number -> value name.
    pub enums: HashMap<String, HashMap<i64, String>>,
    /// "/package.Service/Method" -> (request type, response type).
    pub rpcs: HashMap<String, (String, String)>,
}

const SCALARS: &[&str] = &[
    "double", "float", "int32", "int64", "uint32", "uint64", "sint32", "sint64", "fixed32", "fixed64", "sfixed32", "sfixed64", "bool", "string", "bytes",
];

fn tokenize(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let c: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && c.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if ch == '"' || ch == '\'' {
            let q = ch;
            let mut s = String::from(q);
            i += 1;
            while i < c.len() && c[i] != q {
                if c[i] == '\\' {
                    i += 1;
                }
                if i < c.len() {
                    s.push(c[i]);
                }
                i += 1;
            }
            i += 1;
            out.push(s);
        } else if ch.is_alphanumeric() || ch == '_' || ch == '.' || ch == '-' || ch == '+' {
            let mut s = String::new();
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '.' || ((c[i] == '-' || c[i] == '+') && s.is_empty())) {
                s.push(c[i]);
                i += 1;
            }
            out.push(s);
        } else {
            out.push(ch.to_string());
            i += 1;
        }
    }
    out
}

struct P {
    t: Vec<String>,
    i: usize,
}

impl P {
    fn peek(&self) -> &str {
        self.t.get(self.i).map(|s| s.as_str()).unwrap_or("")
    }
    fn next(&mut self) -> String {
        let s = self.peek().to_string();
        self.i += 1;
        s
    }
    fn done(&self) -> bool {
        self.i >= self.t.len()
    }
    fn expect(&mut self, s: &str) -> Result<(), String> {
        let n = self.next();
        if n == s { Ok(()) } else { Err(format!("expected '{s}' but found '{n}'")) }
    }
    /// Skip to the end of a statement (`;`) or a balanced `{ }` block.
    fn skip_statement(&mut self) {
        let mut depth = 0;
        while !self.done() {
            let n = self.next();
            match n.as_str() {
                "{" => depth += 1,
                "}" => {
                    depth -= 1;
                    if depth <= 0 {
                        return;
                    }
                }
                ";" if depth == 0 => return,
                _ => {}
            }
        }
    }
    /// Skip `[ ... ]` field options.
    fn skip_options(&mut self) {
        if self.peek() == "[" {
            while !self.done() && self.next() != "]" {}
        }
    }
}

/// Raw fields before type resolution: (scope of the message, field).
type RawMsgs = Vec<(String, String, Vec<Field>)>;

fn parse_enum(p: &mut P, full: &str, enums: &mut HashMap<String, HashMap<i64, String>>) -> Result<(), String> {
    p.expect("{")?;
    let mut vals = HashMap::new();
    while !p.done() && p.peek() != "}" {
        let n = p.next();
        if n == "option" || n == "reserved" {
            p.skip_statement();
            continue;
        }
        if n == ";" {
            continue;
        }
        p.expect("=")?;
        let v: i64 = p.next().parse().map_err(|_| format!("bad enum value for {n}"))?;
        p.skip_options();
        p.expect(";")?;
        vals.entry(v).or_insert(n);
    }
    p.expect("}")?;
    enums.insert(full.to_string(), vals);
    Ok(())
}

fn parse_message(p: &mut P, full: &str, msgs: &mut RawMsgs, enums: &mut HashMap<String, HashMap<i64, String>>) -> Result<(), String> {
    p.expect("{")?;
    let mut fields = Vec::new();
    parse_body(p, full, &mut fields, msgs, enums)?;
    p.expect("}")?;
    msgs.push((full.to_string(), full.to_string(), fields));
    Ok(())
}

fn parse_body(p: &mut P, full: &str, fields: &mut Vec<Field>, msgs: &mut RawMsgs, enums: &mut HashMap<String, HashMap<i64, String>>) -> Result<(), String> {
    while !p.done() && p.peek() != "}" {
        let w = p.next();
        match w.as_str() {
            ";" => {}
            "message" => {
                let name = p.next();
                parse_message(p, &format!("{full}.{name}"), msgs, enums)?;
            }
            "enum" => {
                let name = p.next();
                parse_enum(p, &format!("{full}.{name}"), enums)?;
            }
            "oneof" => {
                let _name = p.next();
                p.expect("{")?;
                parse_body(p, full, fields, msgs, enums)?;
                p.expect("}")?;
            }
            "option" | "reserved" | "extensions" | "extend" => p.skip_statement(),
            "map" => {
                p.expect("<")?;
                let k = p.next();
                p.expect(",")?;
                let v = p.next();
                p.expect(">")?;
                let name = p.next();
                p.expect("=")?;
                let number = p.next().parse().map_err(|_| format!("bad field number for {name}"))?;
                p.skip_options();
                p.expect(";")?;
                fields.push(Field { name, number, ty: String::new(), repeated: true, map: Some((k, v)) });
            }
            _ => {
                let (repeated, ty) = match w.as_str() {
                    "repeated" => (true, p.next()),
                    "optional" | "required" => (false, p.next()),
                    _ => (false, w.clone()),
                };
                let name = p.next();
                p.expect("=")?;
                let number = p.next().parse().map_err(|_| format!("bad field number for {name}"))?;
                p.skip_options();
                p.expect(";")?;
                fields.push(Field { name, number, ty, repeated, map: None });
            }
        }
    }
    Ok(())
}

/// Resolve a type name used inside `scope` (a full message name or the package) to a full name.
fn resolve(name: &str, scope: &str, known: &dyn Fn(&str) -> bool) -> String {
    if SCALARS.contains(&name) {
        return name.to_string();
    }
    if let Some(abs) = name.strip_prefix('.') {
        return abs.to_string();
    }
    let mut s = scope.to_string();
    loop {
        let cand = if s.is_empty() { name.to_string() } else { format!("{s}.{name}") };
        if known(&cand) {
            return cand;
        }
        match s.rfind('.') {
            Some(i) => s.truncate(i),
            None if !s.is_empty() => s.clear(),
            None => return name.to_string(),
        }
    }
}

/// Parse one .proto file into `schema` (types keep their package prefix).
pub fn parse_into(schema: &mut Schema, src: &str) -> Result<(), String> {
    let mut p = P { t: tokenize(src), i: 0 };
    let mut package = String::new();
    let mut msgs: RawMsgs = Vec::new();
    let mut rpcs_raw: Vec<(String, String, String)> = Vec::new();
    while !p.done() {
        let w = p.next();
        match w.as_str() {
            "package" => {
                package = p.next();
                p.expect(";")?;
            }
            "syntax" | "edition" | "import" | "option" => p.skip_statement(),
            "message" => {
                let name = p.next();
                let full = if package.is_empty() { name } else { format!("{package}.{name}") };
                parse_message(&mut p, &full, &mut msgs, &mut schema.enums)?;
            }
            "enum" => {
                let name = p.next();
                let full = if package.is_empty() { name } else { format!("{package}.{name}") };
                parse_enum(&mut p, &full, &mut schema.enums)?;
            }
            "service" => {
                let svc = p.next();
                let svc_full = if package.is_empty() { svc } else { format!("{package}.{svc}") };
                p.expect("{")?;
                while !p.done() && p.peek() != "}" {
                    let k = p.next();
                    if k != "rpc" {
                        if k != ";" {
                            p.skip_statement();
                        }
                        continue;
                    }
                    let m = p.next();
                    p.expect("(")?;
                    if p.peek() == "stream" {
                        p.next();
                    }
                    let req = p.next();
                    p.expect(")")?;
                    p.expect("returns")?;
                    p.expect("(")?;
                    if p.peek() == "stream" {
                        p.next();
                    }
                    let resp = p.next();
                    p.expect(")")?;
                    if p.peek() == "{" {
                        p.skip_statement();
                    } else {
                        p.expect(";")?;
                    }
                    rpcs_raw.push((format!("/{svc_full}/{m}"), req, resp));
                }
                p.expect("}")?;
            }
            ";" => {}
            other => return Err(format!("unexpected '{other}'")),
        }
    }
    for (full, _, fields) in &msgs {
        schema.messages.insert(full.clone(), fields.clone());
    }
    let names: std::collections::HashSet<String> = schema.messages.keys().chain(schema.enums.keys()).cloned().collect();
    let known = |n: &str| names.contains(n);
    for (full, scope, _) in &msgs {
        if let Some(fields) = schema.messages.get_mut(full) {
            for f in fields.iter_mut() {
                if let Some((k, v)) = &f.map {
                    f.map = Some((resolve(k, scope, &known), resolve(v, scope, &known)));
                } else {
                    f.ty = resolve(&f.ty, scope, &known);
                }
            }
        }
    }
    for (path, req, resp) in rpcs_raw {
        schema.rpcs.insert(path, (resolve(&req, &package, &known), resolve(&resp, &package, &known)));
    }
    Ok(())
}

pub fn parse(src: &str) -> Result<Schema, String> {
    let mut s = Schema::default();
    parse_into(&mut s, src)?;
    Ok(s)
}

/// Load several .proto files (or every .proto in a folder) into one schema.
pub fn load_files(paths: &[PathBuf]) -> Result<Schema, String> {
    let mut s = Schema::default();
    for p in paths {
        let files: Vec<PathBuf> = if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(p)
                .map_err(|e| format!("{}: {e}", p.display()))?
                .flatten()
                .map(|e| e.path())
                .filter(|f| f.extension().is_some_and(|x| x.eq_ignore_ascii_case("proto")))
                .collect();
            v.sort();
            v
        } else {
            vec![p.clone()]
        };
        for f in files {
            let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
            parse_into(&mut s, &text).map_err(|e| format!("{}: {e}", f.display()))?;
        }
    }
    Ok(s)
}

// ------------------------------------------------------------------------------------------ decoding

fn varint(b: &[u8], p: &mut usize) -> Option<u64> {
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

fn scalar_from_varint(ty: &str, v: u64, schema: &Schema) -> String {
    match ty {
        "bool" => (v != 0).to_string(),
        "int32" => (v as i64 as i32).to_string(),
        "int64" => (v as i64).to_string(),
        "sint32" | "sint64" => (((v >> 1) as i64) ^ -((v & 1) as i64)).to_string(),
        "uint32" | "uint64" => v.to_string(),
        e => match schema.enums.get(e).and_then(|m| m.get(&(v as i64 as i32 as i64))) {
            Some(n) => n.clone(),
            None => (v as i64).to_string(),
        },
    }
}

fn quote(s: &str) -> String {
    format!("{s:?}")
}

/// Decode `b` as message `ty` into indented "name: value" lines.
pub fn decode_message(schema: &Schema, ty: &str, b: &[u8], indent: usize) -> Result<String, String> {
    let fields = schema.messages.get(ty).ok_or_else(|| format!("unknown message type {ty}"))?;
    let pad = "  ".repeat(indent);
    let mut out = String::new();
    let mut p = 0;
    while p < b.len() {
        let key = varint(b, &mut p).ok_or("truncated field key")?;
        let (num, wt) = (key >> 3, (key & 7) as u8);
        let f = fields.iter().find(|f| f.number == num);
        let label = f.map(|f| f.name.clone()).unwrap_or_else(|| format!("{num}"));
        match wt {
            0 => {
                let v = varint(b, &mut p).ok_or("truncated varint")?;
                let shown = f.map(|f| scalar_from_varint(&f.ty, v, schema)).unwrap_or_else(|| v.to_string());
                out.push_str(&format!("{pad}{label}: {shown}\n"));
            }
            1 => {
                let s: [u8; 8] = b.get(p..p + 8).ok_or("truncated fixed64")?.try_into().map_err(|_| "bad fixed64")?;
                p += 8;
                let shown = match f.map(|f| f.ty.as_str()) {
                    Some("double") => f64::from_le_bytes(s).to_string(),
                    Some("sfixed64") => i64::from_le_bytes(s).to_string(),
                    _ => u64::from_le_bytes(s).to_string(),
                };
                out.push_str(&format!("{pad}{label}: {shown}\n"));
            }
            5 => {
                let s: [u8; 4] = b.get(p..p + 4).ok_or("truncated fixed32")?.try_into().map_err(|_| "bad fixed32")?;
                p += 4;
                let shown = match f.map(|f| f.ty.as_str()) {
                    Some("float") => f32::from_le_bytes(s).to_string(),
                    Some("sfixed32") => i32::from_le_bytes(s).to_string(),
                    _ => u32::from_le_bytes(s).to_string(),
                };
                out.push_str(&format!("{pad}{label}: {shown}\n"));
            }
            2 => {
                let n = varint(b, &mut p).ok_or("truncated length")? as usize;
                let s = b.get(p..p.checked_add(n).ok_or("bad length")?).ok_or("truncated bytes")?;
                p += n;
                match f {
                    Some(f) if f.map.is_some() => {
                        let (_, vt) = f.map.clone().unwrap();
                        // map entry: key = 1, value = 2
                        let mut q = 0;
                        let (mut k, mut v) = (String::new(), String::new());
                        while q < s.len() {
                            let key = varint(s, &mut q).ok_or("bad map entry")?;
                            let ewt = key & 7;
                            let part = match ewt {
                                0 => scalar_from_varint(&vt, varint(s, &mut q).ok_or("bad map entry")?, schema),
                                2 => {
                                    let m = varint(s, &mut q).ok_or("bad map entry")? as usize;
                                    let d = s.get(q..q + m).ok_or("bad map entry")?;
                                    q += m;
                                    if key >> 3 == 2 && schema.messages.contains_key(&vt) {
                                        format!("{{\n{}{pad}  }}", decode_message(schema, &vt, d, indent + 2)?)
                                    } else {
                                        quote(&String::from_utf8_lossy(d))
                                    }
                                }
                                _ => return Err("unsupported map entry".into()),
                            };
                            if key >> 3 == 1 { k = part } else { v = part }
                        }
                        out.push_str(&format!("{pad}{label}[{k}]: {v}\n"));
                    }
                    Some(f) if f.ty == "string" => out.push_str(&format!("{pad}{label}: {}\n", quote(&String::from_utf8_lossy(s)))),
                    Some(f) if f.ty == "bytes" => out.push_str(&format!("{pad}{label}: bytes {}\n", crate::ws::hex(s))),
                    Some(f) if schema.messages.contains_key(&f.ty) => {
                        out.push_str(&format!("{pad}{label} {{\n{}{pad}}}\n", decode_message(schema, &f.ty, s, indent + 1)?));
                    }
                    Some(f) if f.repeated => {
                        // packed scalars
                        let mut q = 0;
                        let mut vals = Vec::new();
                        while q < s.len() {
                            match f.ty.as_str() {
                                "double" | "fixed64" | "sfixed64" => {
                                    let x: [u8; 8] = s.get(q..q + 8).ok_or("bad packed")?.try_into().map_err(|_| "bad packed")?;
                                    q += 8;
                                    vals.push(if f.ty == "double" { f64::from_le_bytes(x).to_string() } else { u64::from_le_bytes(x).to_string() });
                                }
                                "float" | "fixed32" | "sfixed32" => {
                                    let x: [u8; 4] = s.get(q..q + 4).ok_or("bad packed")?.try_into().map_err(|_| "bad packed")?;
                                    q += 4;
                                    vals.push(if f.ty == "float" { f32::from_le_bytes(x).to_string() } else { u32::from_le_bytes(x).to_string() });
                                }
                                _ => vals.push(scalar_from_varint(&f.ty, varint(s, &mut q).ok_or("bad packed")?, schema)),
                            }
                        }
                        out.push_str(&format!("{pad}{label}: [{}]\n", vals.join(", ")));
                    }
                    _ => out.push_str(&format!("{pad}{label}: {}\n", quote(&String::from_utf8_lossy(s)))),
                }
            }
            _ => return Err(format!("unsupported wire type {wt}")),
        }
    }
    Ok(out)
}

/// Message types for a gRPC call path (`/pkg.Service/Method`); None when the schema has no such rpc.
pub fn types_for_path<'a>(schema: &'a Schema, path: &str) -> Option<&'a (String, String)> {
    let p = path.split('?').next().unwrap_or(path);
    schema.rpcs.get(p).or_else(|| schema.rpcs.iter().find(|(k, _)| p.ends_with(k.as_str())).map(|(_, v)| v))
}

/// gRPC frames decoded with the schema; Err to fall back to the schema-less view.
pub fn grpc_view_typed(schema: &Schema, path: &str, body: &[u8], response: bool) -> Result<String, String> {
    let (req, resp) = types_for_path(schema, path).ok_or("no matching rpc in the loaded .proto files")?;
    let ty = if response { resp } else { req };
    let frames = crate::streams_decode::grpc_frames(body)?;
    let mut s = String::new();
    for (i, f) in frames.iter().enumerate() {
        s.push_str(&format!("Message {} ({ty}) - compressed: {}, length: {}\n", i + 1, f.compressed, f.length));
        if f.compressed {
            s.push_str("  (compressed with the grpc-encoding codec)\n");
        } else {
            s.push_str(&decode_message(schema, ty, &f.data, 1)?);
        }
    }
    Ok(s)
}

// ------------------------------------------------------------------------------------------ settings

/// The .proto files / folders to load (data/protos.json, env FERROTAP_PROTOS).
pub fn settings_path() -> PathBuf {
    std::env::var("FERROTAP_PROTOS").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data").join("protos.json"))
}

pub fn load_paths(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_paths(p: &Path, paths: &[String]) -> Result<(), String> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(p, serde_json::to_string_pretty(paths).unwrap_or_default()).map_err(|e| e.to_string())
}

/// Protobuf settings state (GridApp.ext.proto).
#[derive(Default)]
pub struct ProtoUi {
    pub paths: Option<Vec<String>>,
    pub schema: Option<Schema>,
    pub msg: String,
    pub new_path: String,
    /// Server reflection: address and the background result.
    pub reflect_url: String,
    pub reflect_rx: Option<std::sync::mpsc::Receiver<Result<(Schema, Vec<String>), String>>>,
}

impl Schema {
    /// Add everything from `other` (later definitions win).
    pub fn merge(&mut self, other: Schema) {
        self.messages.extend(other.messages);
        self.enums.extend(other.enums);
        self.rpcs.extend(other.rpcs);
    }
}

impl ProtoUi {
    /// (Re)load the schema from the saved paths.
    pub fn reload(&mut self) {
        let paths = self.paths.get_or_insert_with(|| load_paths(&settings_path())).clone();
        let list: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        match load_files(&list) {
            Ok(s) => {
                self.msg = format!("{} messages, {} services methods loaded", s.messages.len(), s.rpcs.len());
                self.schema = Some(s);
            }
            Err(e) => {
                self.msg = e;
                self.schema = None;
            }
        }
    }

    pub fn schema(&mut self) -> Option<&Schema> {
        if self.schema.is_none() && self.paths.is_none() {
            self.reload();
        }
        self.schema.as_ref()
    }
}

impl crate::ui::GridApp {
    /// Protobuf settings: the .proto files / folders used to decode gRPC with field names.
    pub fn proto_settings_ui(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui::{self, Color32, RichText};
        let mut pick = false;
        let st = &mut self.ext.proto;
        if st.paths.is_none() {
            st.reload();
        }
        egui::CollapsingHeader::new(format!("Protobuf (.proto files: {})", st.paths.as_ref().map_or(0, |p| p.len()))).id_salt("proto_settings").show(ui, |ui| {
            let mut changed = false;
            let mut del = None;
            for (i, p) in st.paths.clone().unwrap_or_default().iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.monospace(p);
                    if ui.small_button("x").clicked() {
                        del = Some(i);
                    }
                });
            }
            if let Some(i) = del {
                st.paths.get_or_insert_with(Vec::new).remove(i);
                changed = true;
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut st.new_path).hint_text("a .proto file or a folder of them").desired_width(280.0));
                if ui.small_button("Browse...").clicked() {
                    pick = true;
                }
                if ui.small_button("Add").clicked() && !st.new_path.trim().is_empty() {
                    let p = st.new_path.trim().trim_matches('"').to_string();
                    st.paths.get_or_insert_with(Vec::new).push(p);
                    st.new_path.clear();
                    changed = true;
                }
                if ui.small_button("Reload").clicked() {
                    st.reload();
                }
            });
            if changed {
                let _ = crate::protoschema::save_paths(&crate::protoschema::settings_path(), st.paths.as_deref().unwrap_or(&[]));
                st.reload();
            }
            if !st.msg.is_empty() {
                ui.label(RichText::new(&st.msg).small().color(Color32::GRAY));
            }
            ui.label(RichText::new("Calls are matched by path (/package.Service/Method) to the rpc definitions in the files.").small().color(Color32::GRAY));
            ui.horizontal(|ui| {
                ui.label("Server reflection");
                ui.add(egui::TextEdit::singleline(&mut st.reflect_url).hint_text("http://localhost:50051 or https://...").desired_width(240.0));
                let busy = st.reflect_rx.is_some();
                if ui.add_enabled(!busy && !st.reflect_url.trim().is_empty(), egui::Button::new(if busy { "Loading..." } else { "Load from server" })).clicked() {
                    let url = st.reflect_url.trim().to_string();
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = tx.send(crate::grpc_reflect::reflect_blocking(&url));
                    });
                    st.reflect_rx = Some(rx);
                }
            });
        });
        if let Some(rx) = &st.reflect_rx {
            match rx.try_recv() {
                Ok(Ok((s, services))) => {
                    st.schema.get_or_insert_with(Schema::default).merge(s);
                    st.msg = format!("Reflection: {} service(s) loaded ({})", services.len(), services.join(", "));
                    st.reflect_rx = None;
                }
                Ok(Err(e)) => {
                    st.msg = e;
                    st.reflect_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)),
                Err(_) => st.reflect_rx = None,
            }
        }
        if pick {
            let start = st.new_path.clone();
            self.ext.pick.open_file("proto_file", "Choose a .proto file", &start, &["proto"]);
        }
        if let Some(p) = self.ext.pick.take("proto_file") {
            self.ext.proto.new_path = p.display().to_string();
        }
    }
}
