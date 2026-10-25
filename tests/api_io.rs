//! G15: Postman v2.1 and OpenAPI 3 round trips, collection from captured traffic.
use ferrotap::api_tools::*;
use ferrotap::model::{Collection, KvRow, SavedRequest, SessionRecord};

fn sample() -> Collection {
    let mut c = Collection::new("Shop API");
    c.requests.push(SavedRequest {
        name: "List items".into(), method: "GET".into(), url: "{{baseUrl}}/items".into(),
        params: vec![KvRow::new("page", "1"), KvRow { enabled: false, key: "debug".into(), value: "1".into() }],
        headers: vec![KvRow::new("Accept", "application/json")],
        tests: vec![Assertion::new(AssertKind::StatusEquals, "", "200")],
        example: Some(SavedResponse { status: 200, headers: vec![("Content-Type".into(), "application/json".into())], body: "[{\"id\":1}]".into() }),
        ..Default::default()
    });
    c.requests.push(SavedRequest {
        name: "Create item".into(), method: "POST".into(), url: "{{baseUrl}}/items/{{id}}".into(),
        headers: vec![KvRow::new("Content-Type", "application/json")], body: "{\"name\":\"x\"}".into(), ..Default::default()
    });
    c.requests.push(SavedRequest {
        name: "Gql".into(), method: "POST".into(), url: "http://h/graphql".into(),
        body: build_graphql_body("{ a }", "{\"v\":1}", "").unwrap(), body_mode: "graphql".into(), ..Default::default()
    });
    c
}

#[test]
fn postman_round_trip() {
    let c = sample();
    let v = to_postman(&c, &[("baseUrl".into(), "http://127.0.0.1:1".into())]);
    assert_eq!(v["info"]["schema"], POSTMAN_SCHEMA);
    assert_eq!(v["item"][0]["request"]["url"]["raw"], "{{baseUrl}}/items?page=1");
    assert_eq!(v["item"][2]["request"]["body"]["mode"], "graphql");
    let text = serde_json::to_string(&v).unwrap();
    let (back, vars) = from_postman(&text).unwrap();
    assert_eq!(back.name, "Shop API");
    assert_eq!(vars, vec![KvRow::new("baseUrl", "http://127.0.0.1:1")]);
    assert_eq!(back.requests.len(), 3);
    for (a, b) in c.requests.iter().zip(back.requests.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.method, b.method);
        assert_eq!(a.url, b.url);
        assert_eq!(a.params, b.params);
        assert_eq!(a.headers, b.headers);
        assert_eq!(a.tests, b.tests);
        assert_eq!(a.body_mode, b.body_mode);
        if a.body.is_empty() {
            assert!(b.body.is_empty());
        } else {
            let (ja, jb): (serde_json::Value, serde_json::Value) = (serde_json::from_str(&a.body).unwrap(), serde_json::from_str(&b.body).unwrap());
            assert_eq!(ja, jb);
        }
    }
    assert_eq!(back.requests[0].example, c.requests[0].example);
}

#[test]
fn postman_import_folders_and_body_modes() {
    let text = r#"{"info":{"name":"P","schema":"x"},"item":[
        {"name":"F","item":[{"name":"a","request":{"method":"get","url":{"protocol":"http","host":["example","com"],"path":["v1","x"],"query":[{"key":"q","value":"1"}]}}}]},
        {"name":"b","request":"http://h/plain?z=2"},
        {"name":"c","request":{"method":"POST","url":"http://h/f","body":{"mode":"urlencoded","urlencoded":[{"key":"a b","value":"1"},{"key":"x","value":"y","disabled":true}]}}}
    ]}"#;
    let (c, _) = from_postman(text).unwrap();
    assert_eq!(c.requests[0].name, "F/a");
    assert_eq!(c.requests[0].method, "GET");
    assert_eq!(c.requests[0].url, "http://example.com/v1/x");
    assert_eq!(c.requests[0].params, vec![KvRow::new("q", "1")]);
    assert_eq!(c.requests[1].url, "http://h/plain");
    assert_eq!(c.requests[2].body, "a%20b=1");
    assert!(from_postman("{\"x\":1}").is_err());
}

#[test]
fn openapi_round_trip() {
    let c = sample();
    let v = to_openapi(&c);
    assert_eq!(v["openapi"], "3.0.3");
    assert!(v["paths"]["/items"]["get"].is_object());
    assert!(v["paths"]["/items/{id}"]["post"]["requestBody"]["content"]["application/json"]["example"]["name"] == "x");
    assert_eq!(v["paths"]["/items"]["get"]["responses"]["200"]["content"]["application/json"]["example"][0]["id"], 1);
    let back = from_openapi(&serde_json::to_string(&v).unwrap()).unwrap();
    assert_eq!(back.name, "Shop API");
    assert_eq!(back.requests.len(), 3);
    let get = back.requests.iter().find(|r| r.method == "GET").unwrap();
    assert_eq!(url_path(&get.url), "/items");
    assert!(get.params.iter().any(|p| p.key == "page" && p.value == "1"));
    assert!(get.headers.iter().any(|h| h.key == "Accept"));
    assert_eq!(get.example.as_ref().unwrap().status, 200);
    let post = back.requests.iter().find(|r| url_path(&r.url) == "/items/{{id}}").unwrap();
    assert_eq!(post.method, "POST");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&post.body).unwrap()["name"], "x");
}

#[test]
fn openapi_import_spec() {
    let spec = r#"{"openapi":"3.0.0","info":{"title":"Pets"},"servers":[{"url":"http://{host}/v1","variables":{"host":{"default":"pets.local"}}}],
      "paths":{"/pets/{petId}":{"parameters":[{"name":"petId","in":"path","required":true}],
        "get":{"operationId":"getPet","parameters":[{"name":"verbose","in":"query","schema":{"type":"boolean","example":true}},{"name":"X-Key","in":"header","example":"k"}],
               "responses":{"200":{"description":"ok","content":{"application/json":{"example":{"id":1}}}}}},
        "delete":{"summary":"Delete pet","responses":{"204":{"description":"gone"}}}}}}"#;
    let c = from_openapi(spec).unwrap();
    assert_eq!(c.name, "Pets");
    let g = c.find("getPet").unwrap();
    assert_eq!(g.url, "http://pets.local/v1/pets/{{petId}}");
    assert_eq!(g.params, vec![KvRow::new("verbose", "true")]);
    assert_eq!(g.headers, vec![KvRow::new("X-Key", "k")]);
    assert_eq!(g.example.as_ref().unwrap().body.replace([' ', '\n'], ""), "{\"id\":1}");
    assert_eq!(c.find("Delete pet").unwrap().method, "DELETE");
    assert!(from_openapi("{\"a\":1}").is_err());
}

#[test]
fn collection_from_captured_traffic() {
    let mut a = SessionRecord { id: 1, method: "GET".into(), url: "http://h/api/users?limit=5".into(), status: 200, response_body: "[1]".into(), ..Default::default() };
    a.request_headers.insert("Host".into(), "h".into());
    a.request_headers.insert("Authorization".into(), "Bearer t".into());
    a.response_headers.insert("Content-Type".into(), "application/json".into());
    a.response_headers.insert("Content-Length".into(), "3".into());
    let b = SessionRecord { id: 2, method: "GET".into(), url: "http://h/api/users?limit=9".into(), status: 200, ..Default::default() };
    let g = SessionRecord { id: 3, method: "POST".into(), url: "http://h/graphql".into(), request_body: "{\"query\":\"{ a }\"}".into(), status: 200, ..Default::default() };
    let t = SessionRecord { id: 4, method: "CONNECT".into(), url: "h:443".into(), ..Default::default() };
    let c = collection_from_sessions("Cap", &[&a, &b, &g, &t]);
    assert_eq!(c.requests.len(), 3);
    assert_eq!(c.requests[0].name, "GET /api/users");
    assert_eq!(c.requests[1].name, "GET /api/users (2)");
    assert_eq!(c.requests[0].url, "http://h/api/users");
    assert_eq!(c.requests[0].params, vec![KvRow::new("limit", "5")]);
    assert_eq!(c.requests[0].headers, vec![KvRow::new("Authorization", "Bearer t")]);
    let ex = c.requests[0].example.as_ref().unwrap();
    assert_eq!((ex.status, ex.body.as_str()), (200, "[1]"));
    assert_eq!(ex.headers, vec![("Content-Type".to_string(), "application/json".to_string())]);
    assert_eq!(c.requests[2].body_mode, "graphql");
}
