use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde_json::{Map, Value, json};

use crate::proto::{PROTOCOL, Request, Response};
use crate::results::*;

type SchemaFn = fn(&mut SchemaGenerator) -> Schema;

fn one<T: JsonSchema>(g: &mut SchemaGenerator) -> Schema {
    g.subschema_for::<T>()
}

fn list<T: JsonSchema>(g: &mut SchemaGenerator) -> Schema {
    g.subschema_for::<Vec<T>>()
}

pub const RESULTS: &[(&str, SchemaFn)] = &[
    ("ping", one::<Pong>),
    ("ws.register", one::<WsRegistered>),
    ("ws.list", list::<WsInfo>),
    ("ws.detach", one::<WsDetached>),
    ("ws.prune", one::<Pruned>),
    ("changeset", one::<LiveChangeset>),
    ("reset", one::<Applied>),
    ("revert", one::<Applied>),
    ("base.promote", one::<BaseChanged>),
    ("base.flatten", one::<BaseChanged>),
    ("session.create", one::<SessionCreated>),
    ("session.list", list::<SessionInfo>),
    ("session.merge", one::<SessionEnded>),
    ("session.discard", one::<SessionEnded>),
    ("proc.run", one::<ProcStarted>),
    ("proc.changeset", one::<ProcChangesetTaken>),
    ("proc.merge", one::<ProcMerged>),
    ("proc.kill", one::<ProcKilled>),
    ("proc.show", one::<ProcInfo>),
    ("proc.list", list::<ProcInfo>),
    ("log", list::<ChangesetInfo>),
    ("diff", list::<DiffResult>),
    ("push", one::<Pushed>),
    ("pull", one::<Pulled>),
    ("fs.read", one::<FsFile>),
    ("fs.list", one::<FsListing>),
    ("fs.stat", one::<FsEntry>),
    ("fs.edit", one::<FsEdited>),
];

pub fn protocol() -> Value {
    let mut g = SchemaGenerator::default();
    let request = g.subschema_for::<Request>();
    let response = g.subschema_for::<Response>();
    let proc_exited = g.subschema_for::<ProcExited>();
    let mut results = Map::new();
    for (op, f) in RESULTS {
        results.insert(op.to_string(), f(&mut g).to_value());
    }
    let ops: Vec<&str> = RESULTS.iter().map(|(op, _)| *op).collect();
    let mut defs = g.take_definitions(true);
    name_requests(&mut defs);
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "TracefsProtocol",
        "description": "tracefs-daemon socket protocol: one JSON request line and one JSON response line per connection\n(`proc.run`: a second line, `ProcExited`).",
        "type": "object",
        "properties": {
            "protocol": { "const": PROTOCOL },
            "request": request,
            "response": response,
            "proc_exited": proc_exited,
            "results": { "type": "object", "properties": results, "required": ops },
        },
        "required": ["protocol", "request", "response", "proc_exited", "results"],
        "$defs": defs,
    })
}

fn name_requests(defs: &mut Map<String, Value>) {
    let Some(Value::Array(variants)) = defs.get_mut("Request").and_then(|r| r.get_mut("oneOf")) else { return };
    for v in variants {
        let op = v["properties"]["op"]["const"].as_str().unwrap_or_default();
        let name: String = op
            .split('.')
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
            })
            .collect();
        v["title"] = json!(format!("{name}Request"));
        if let Some(args) = v["properties"].get_mut("args")
            && args.get("$ref").is_none()
        {
            args["title"] = json!(format!("{name}Args"));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::*;

    fn request_ops(v: &Value) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for variant in v["$defs"]["Request"]["oneOf"].as_array().unwrap() {
            out.insert(variant["properties"]["op"]["const"].as_str().unwrap().to_string());
        }
        out
    }

    #[test]
    fn every_op_has_a_result() {
        let v = protocol();
        let results: BTreeSet<String> = RESULTS.iter().map(|(op, _)| op.to_string()).collect();
        assert_eq!(request_ops(&v), results);
    }

    #[test]
    fn schema_file_is_current() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/tracefs.schema.json");
        let want = serde_json::to_string_pretty(&protocol()).unwrap() + "\n";
        if std::env::var_os("UPDATE_SCHEMA").is_some() {
            std::fs::write(&file, &want).unwrap();
        }
        let have = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(have == want, "{} is out of date: run `UPDATE_SCHEMA=1 cargo test -p tracefs-core schema`", file.display());
    }
}
