use anyhow::{Context, Result, anyhow};
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::io::{self, BufRead, BufReader, Write};

use crate::db;

const JSONRPC_VERSION: &str = "2.0";
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const RESOURCE_URIS: [(&str, &str); 4] = [
    ("mem://facts", "facts"),
    ("mem://decisions", "decisions"),
    ("mem://modules", "modules"),
    ("mem://commands", "commands"),
];

pub fn serve_stdio() -> Result<()> {
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout.lock());

    loop {
        let Some(request) = read_message(&mut reader)? else {
            break;
        };

        if let Some(response) = handle_request(&request)? {
            write_message(&mut writer, &response)?;
            writer.flush().context("failed to flush stdout")?;
        }
    }

    Ok(())
}

fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<Value>> {
    let first_non_whitespace = loop {
        let buffer = reader
            .fill_buf()
            .context("failed to peek MCP message bytes")?;
        if buffer.is_empty() {
            return Ok(None);
        }

        let mut consumed = 0usize;
        while consumed < buffer.len() && matches!(buffer[consumed], b' ' | b'\t' | b'\r' | b'\n') {
            consumed += 1;
        }

        if consumed > 0 {
            reader.consume(consumed);
            continue;
        }

        break buffer[0];
    };

    if first_non_whitespace == b'{' || first_non_whitespace == b'[' {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .context("failed to read MCP JSON line")?;
        if bytes == 0 {
            return Ok(None);
        }
        let message =
            serde_json::from_str(line.trim()).context("failed to parse MCP JSON message")?;
        return Ok(Some(message));
    }

    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .context("failed to read MCP header line")?;
        if bytes == 0 {
            return Ok(None);
        }

        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }

        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            let value = value.trim();
            content_length = Some(
                value
                    .parse::<usize>()
                    .with_context(|| format!("invalid Content-Length value: {value}"))?,
            );
        }
    }

    let len = content_length.ok_or_else(|| anyhow!("missing Content-Length header"))?;
    let mut body = vec![0u8; len];
    reader
        .read_exact(&mut body)
        .context("failed to read MCP message body")?;

    let message: Value =
        serde_json::from_slice(&body).context("failed to parse MCP JSON message")?;
    Ok(Some(message))
}

fn write_message<W: Write>(writer: &mut W, message: &Value) -> Result<()> {
    let body = serde_json::to_vec(message).context("failed to serialize MCP response")?;
    writer
        .write_all(&body)
        .context("failed to write MCP response body")?;
    writer
        .write_all(b"\n")
        .context("failed to write MCP response newline")?;
    Ok(())
}

fn handle_request(request: &Value) -> Result<Option<Value>> {
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("MCP message missing method"))?;
    let id = request.get("id").cloned();
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

    if id.is_none() {
        if method == "notifications/initialized" {
            return Ok(None);
        }
        return Ok(None);
    }

    let response = match method {
        "initialize" => success(
            id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {},
                    "resources": {},
                },
                "serverInfo": {
                    "name": "mem-cli",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
        ),
        "tools/list" => success(id, json!({ "tools": tool_specs() })),
        "tools/call" => handle_tool_call(id, &params),
        "resources/list" => success(id, json!({ "resources": resource_specs() })),
        "resources/read" => handle_resource_read(id, &params),
        _ => error(id, -32601, &format!("method not found: {method}")),
    };

    Ok(Some(response))
}

fn handle_tool_call(id: Option<Value>, params: &Value) -> Value {
    let name = match params.get("name").and_then(Value::as_str) {
        Some(n) if !n.is_empty() => n,
        _ => return error(id, -32602, "tools/call requires non-empty field: name"),
    };
    let args = params.get("arguments").unwrap_or(&Value::Null);

    let conn = match db::open().and_then(|conn| {
        db::apply_migrations(&conn)?;
        Ok(conn)
    }) {
        Ok(conn) => conn,
        Err(e) => return error(id, -32603, &format!("failed to open DB: {e}")),
    };

    match execute_tool(&conn, name, args) {
        Ok(structured) => success(
            id,
            json!({
                "content": [{ "type": "text", "text": "ok" }],
                "structuredContent": structured,
                "isError": false
            }),
        ),
        Err(ToolError::InvalidParams(message)) => error(id, -32602, &message),
        Err(ToolError::Internal(err)) => error(id, -32603, &err.to_string()),
    }
}

fn handle_resource_read(id: Option<Value>, params: &Value) -> Value {
    let args = match params.as_object() {
        Some(v) => v,
        None => return error(id, -32602, "resources/read params must be an object"),
    };
    let uri = match args.get("uri").and_then(Value::as_str) {
        Some(v) if !v.is_empty() => v,
        _ => return error(id, -32602, "resources/read requires non-empty field: uri"),
    };
    let Some(entity) = entity_from_uri(uri) else {
        return error(id, -32602, &format!("unknown resource uri: {uri}"));
    };

    let conn = match db::open().and_then(|conn| {
        db::apply_migrations(&conn)?;
        Ok(conn)
    }) {
        Ok(conn) => conn,
        Err(e) => return error(id, -32603, &format!("failed to open DB: {e}")),
    };

    let records = match db::list_active(&conn, entity) {
        Ok(records) => records,
        Err(e) => return error(id, -32603, &e.to_string()),
    };
    let payload = json!({"entity": entity, "records": records_to_json(&records)});
    let text = match serde_json::to_string(&payload) {
        Ok(text) => text,
        Err(e) => {
            return error(
                id,
                -32603,
                &format!("failed to serialize resource payload: {e}"),
            );
        }
    };
    success(
        id,
        json!({
            "contents": [{
                "uri": uri,
                "mimeType": "application/json",
                "text": text
            }]
        }),
    )
}

#[derive(Debug)]
enum ToolError {
    InvalidParams(String),
    Internal(anyhow::Error),
}

fn invalid_params(message: impl Into<String>) -> ToolError {
    ToolError::InvalidParams(message.into())
}

fn tool_specs() -> Vec<Value> {
    vec![
        json!({
            "name": "ping",
            "description": "Health check for the MCP transport.",
            "inputSchema": {"type":"object","properties":{},"additionalProperties":false}
        }),
        json!({
            "name": "list_facts",
            "description": "List active facts.",
            "inputSchema": {"type":"object","properties":{},"additionalProperties":false}
        }),
        json!({
            "name": "list_decisions",
            "description": "List active decisions.",
            "inputSchema": {"type":"object","properties":{},"additionalProperties":false}
        }),
        json!({
            "name": "list_modules",
            "description": "List active modules.",
            "inputSchema": {"type":"object","properties":{},"additionalProperties":false}
        }),
        json!({
            "name": "list_commands",
            "description": "List active commands.",
            "inputSchema": {"type":"object","properties":{},"additionalProperties":false}
        }),
        json!({
            "name": "find_by_tag",
            "description": "Find active records by tag across entities.",
            "inputSchema": {
                "type":"object",
                "properties": {
                    "tag": {"type":"string"},
                    "entity": {"type":"string","enum": db::ENTITY_TABLES}
                },
                "required": ["tag"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "add_fact",
            "description": "Add a fact record.",
            "inputSchema": {
                "type":"object",
                "properties":{"content":{"type":"string"}},
                "required":["content"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "add_decision",
            "description": "Add a decision record.",
            "inputSchema": {
                "type":"object",
                "properties":{"content":{"type":"string"}},
                "required":["content"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "add_module",
            "description": "Add a module record.",
            "inputSchema": {
                "type":"object",
                "properties":{"content":{"type":"string"}},
                "required":["content"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "add_command",
            "description": "Add a command record.",
            "inputSchema": {
                "type":"object",
                "properties":{"content":{"type":"string"}},
                "required":["content"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "tag_record",
            "description": "Attach one or more tags to a record.",
            "inputSchema": {
                "type":"object",
                "properties":{
                    "entity":{"type":"string","enum": db::ENTITY_TABLES},
                    "id":{"type":"integer"},
                    "tags":{"type":"array","items":{"type":"string"}}
                },
                "required":["entity","id","tags"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "untag_record",
            "description": "Remove a tag from a record.",
            "inputSchema": {
                "type":"object",
                "properties":{
                    "entity":{"type":"string","enum": db::ENTITY_TABLES},
                    "id":{"type":"integer"},
                    "tag":{"type":"string"},
                    "hard":{"type":"boolean"}
                },
                "required":["entity","id","tag"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "doctor",
            "description": "Diagnose DB and tag integrity; optionally apply safe fix.",
            "inputSchema": {
                "type":"object",
                "properties":{"fix":{"type":"boolean"}},
                "additionalProperties": false
            }
        }),
    ]
}

fn resource_specs() -> Vec<Value> {
    RESOURCE_URIS
        .iter()
        .map(|(uri, entity)| {
            json!({
                "uri": uri,
                "name": format!("mem-cli {entity}"),
                "description": format!("Active {entity} records from mem-cli context."),
                "mimeType": "application/json",
            })
        })
        .collect()
}

fn entity_from_uri(uri: &str) -> Option<&'static str> {
    RESOURCE_URIS
        .iter()
        .find_map(|(u, e)| if *u == uri { Some(*e) } else { None })
}

fn execute_tool(
    conn: &Connection,
    name: &str,
    args: &Value,
) -> std::result::Result<Value, ToolError> {
    match name {
        "ping" => Ok(json!({"ok": true, "message": "pong"})),
        "list_facts" => list_entity(conn, "facts"),
        "list_decisions" => list_entity(conn, "decisions"),
        "list_modules" => list_entity(conn, "modules"),
        "list_commands" => list_entity(conn, "commands"),
        "find_by_tag" => find_by_tag(conn, args),
        "add_fact" => add_to_entity(conn, "facts", args),
        "add_decision" => add_to_entity(conn, "decisions", args),
        "add_module" => add_to_entity(conn, "modules", args),
        "add_command" => add_to_entity(conn, "commands", args),
        "tag_record" => tag_record(conn, args),
        "untag_record" => untag_record(conn, args),
        "doctor" => doctor(conn, args),
        _ => Err(invalid_params(format!("unknown tool: {name}"))),
    }
}

fn list_entity(conn: &Connection, entity: &str) -> std::result::Result<Value, ToolError> {
    let records = db::list_active(conn, entity).map_err(ToolError::Internal)?;
    Ok(json!({"entity": entity, "records": records_to_json(&records)}))
}

fn find_by_tag(conn: &Connection, args: &Value) -> std::result::Result<Value, ToolError> {
    let args = arg_object(args)?;
    let tag = get_required_string(args, "tag")?;
    let entity = get_optional_entity(args, "entity")?;
    let groups = db::find_by_tag(conn, tag, entity).map_err(ToolError::Internal)?;

    let grouped: Map<String, Value> = groups
        .into_iter()
        .map(|(name, records)| (name, records_to_json(&records)))
        .collect();

    Ok(json!({
        "tag": db::normalize_tag(tag).map_err(ToolError::Internal)?,
        "groups": grouped
    }))
}

fn add_to_entity(
    conn: &Connection,
    entity: &str,
    args: &Value,
) -> std::result::Result<Value, ToolError> {
    let args = arg_object(args)?;
    let content = get_required_string(args, "content")?;
    let id = db::insert(conn, entity, content).map_err(ToolError::Internal)?;
    Ok(json!({"entity": entity, "id": id}))
}

fn tag_record(conn: &Connection, args: &Value) -> std::result::Result<Value, ToolError> {
    let args = arg_object(args)?;
    let entity = get_required_entity(args, "entity")?;
    let id = get_required_i64(args, "id")?;
    let tags = get_required_string_array(args, "tags")?;

    let mut outcomes = Vec::with_capacity(tags.len());
    for tag in tags {
        let outcome = db::add_tag(conn, entity, id, &tag).map_err(ToolError::Internal)?;
        outcomes.push(json!({
            "tag": db::normalize_tag(&tag).map_err(ToolError::Internal)?,
            "outcome": match outcome {
                db::TagOutcome::Added => "added",
                db::TagOutcome::AlreadyPresent => "already_present",
                db::TagOutcome::NoRecord => "no_record"
            }
        }));
    }

    Ok(json!({
        "entity": entity,
        "id": id,
        "results": outcomes
    }))
}

fn untag_record(conn: &Connection, args: &Value) -> std::result::Result<Value, ToolError> {
    let args = arg_object(args)?;
    let entity = get_required_entity(args, "entity")?;
    let id = get_required_i64(args, "id")?;
    let tag = get_required_string(args, "tag")?;
    let hard = get_optional_bool(args, "hard").unwrap_or(false);
    let affected = db::remove_tag(conn, entity, id, tag, hard).map_err(ToolError::Internal)?;
    Ok(json!({
        "entity": entity,
        "id": id,
        "tag": db::normalize_tag(tag).map_err(ToolError::Internal)?,
        "hard": hard,
        "affected": affected
    }))
}

fn doctor(conn: &Connection, args: &Value) -> std::result::Result<Value, ToolError> {
    let args = arg_object(args)?;
    let fix = get_optional_bool(args, "fix").unwrap_or(false);
    let report = db::doctor(conn).map_err(ToolError::Internal)?;
    let removed = if fix {
        db::doctor_fix(conn, &report).map_err(ToolError::Internal)?
    } else {
        0
    };
    let after = if fix {
        db::doctor(conn).map_err(ToolError::Internal)?
    } else {
        report
    };
    Ok(json!({
        "report": doctor_report_to_json(&after),
        "fix": fix,
        "removed": removed
    }))
}

fn arg_object(args: &Value) -> std::result::Result<&Map<String, Value>, ToolError> {
    args.as_object()
        .ok_or_else(|| invalid_params("tool arguments must be an object"))
}

fn get_required_string<'a>(
    args: &'a Map<String, Value>,
    field: &str,
) -> std::result::Result<&'a str, ToolError> {
    args.get(field)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| invalid_params(format!("missing or invalid field: {field}")))
}

fn get_required_i64(args: &Map<String, Value>, field: &str) -> std::result::Result<i64, ToolError> {
    args.get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_params(format!("missing or invalid field: {field}")))
}

fn get_optional_bool(args: &Map<String, Value>, field: &str) -> Option<bool> {
    args.get(field).and_then(Value::as_bool)
}

fn get_required_entity<'a>(
    args: &'a Map<String, Value>,
    field: &str,
) -> std::result::Result<&'a str, ToolError> {
    let entity = get_required_string(args, field)?;
    if db::ENTITY_TABLES.contains(&entity) {
        Ok(entity)
    } else {
        Err(invalid_params(format!("unknown entity: {entity}")))
    }
}

fn get_optional_entity<'a>(
    args: &'a Map<String, Value>,
    field: &str,
) -> std::result::Result<Option<&'a str>, ToolError> {
    let Some(value) = args.get(field) else {
        return Ok(None);
    };
    let Some(entity) = value.as_str() else {
        return Err(invalid_params(format!("invalid field type: {field}")));
    };
    if db::ENTITY_TABLES.contains(&entity) {
        Ok(Some(entity))
    } else {
        Err(invalid_params(format!("unknown entity: {entity}")))
    }
}

fn get_required_string_array(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Vec<String>, ToolError> {
    let Some(values) = args.get(field).and_then(Value::as_array) else {
        return Err(invalid_params(format!("missing or invalid field: {field}")));
    };
    if values.is_empty() {
        return Err(invalid_params(format!("field must not be empty: {field}")));
    }
    values
        .iter()
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| invalid_params(format!("field must be array of strings: {field}")))
        })
        .collect()
}

fn records_to_json(records: &[db::Record]) -> Value {
    Value::Array(
        records
            .iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "content": r.content,
                    "created_at": r.created_at,
                    "updated_at": r.updated_at,
                    "deleted_at": r.deleted_at,
                })
            })
            .collect(),
    )
}

fn doctor_report_to_json(report: &db::DoctorReport) -> Value {
    json!({
        "schema_version": report.schema_version,
        "dangling": tag_issues_to_json(&report.dangling),
        "invalid_entity": tag_issues_to_json(&report.invalid_entity),
        "dirty": tag_issues_to_json(&report.dirty),
        "on_soft_deleted": tag_issues_to_json(&report.on_soft_deleted),
        "active_counts": counts_to_json(&report.active_counts),
        "soft_deleted_counts": counts_to_json(&report.soft_deleted_counts),
        "has_problems": report.has_problems(),
    })
}

fn tag_issues_to_json(issues: &[db::TagIssue]) -> Value {
    Value::Array(
        issues
            .iter()
            .map(|i| {
                json!({
                    "id": i.id,
                    "entity": i.entity,
                    "record_id": i.record_id,
                    "tag": i.tag
                })
            })
            .collect(),
    )
}

fn counts_to_json(counts: &[(String, i64)]) -> Value {
    Value::Object(counts.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

fn success(id: Option<Value>, result: Value) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "result": result
    })
}

fn error(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        db::apply_migrations(&conn).expect("apply migrations");
        conn
    }

    #[test]
    fn tools_list_contains_v1_tools() {
        let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}});
        let response = handle_request(&request)
            .expect("handle request")
            .expect("response");
        let tools = response
            .get("result")
            .and_then(|r| r.get("tools"))
            .and_then(Value::as_array)
            .expect("tools array");
        assert!(
            tools
                .iter()
                .any(|t| t.get("name") == Some(&json!("list_facts")))
        );
        assert!(
            tools
                .iter()
                .any(|t| t.get("name") == Some(&json!("doctor")))
        );
    }

    #[test]
    fn add_and_list_fact_tool_flow() {
        let conn = setup_conn();
        let add_result =
            execute_tool(&conn, "add_fact", &json!({"content":"fact-1"})).expect("add");
        assert!(add_result.get("id").and_then(Value::as_i64).is_some());

        let list_result = execute_tool(&conn, "list_facts", &json!({})).expect("list");
        let records = list_result
            .get("records")
            .and_then(Value::as_array)
            .expect("records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].get("content"), Some(&json!("fact-1")));
    }

    #[test]
    fn tag_and_find_by_tag_flow() {
        let conn = setup_conn();
        let add = execute_tool(&conn, "add_fact", &json!({"content":"auth fact"})).expect("add");
        let id = add
            .get("id")
            .and_then(Value::as_i64)
            .expect("id in add response");

        execute_tool(
            &conn,
            "tag_record",
            &json!({"entity":"facts","id":id,"tags":["Auth"]}),
        )
        .expect("tag");
        let found = execute_tool(&conn, "find_by_tag", &json!({"tag":"auth"})).expect("find");
        let groups = found
            .get("groups")
            .and_then(Value::as_object)
            .expect("groups object");
        let facts = groups
            .get("facts")
            .and_then(Value::as_array)
            .expect("facts group");
        assert_eq!(facts.len(), 1);
    }

    #[test]
    fn unknown_tool_returns_invalid_params() {
        let conn = setup_conn();
        let err = execute_tool(&conn, "not_exists", &json!({})).expect_err("expected error");
        match err {
            ToolError::InvalidParams(message) => assert!(message.contains("unknown tool")),
            ToolError::Internal(e) => panic!("unexpected internal error: {e}"),
        }
    }

    #[test]
    fn resources_list_exposes_context_resources() {
        let request = json!({"jsonrpc":"2.0","id":1,"method":"resources/list","params":{}});
        let response = handle_request(&request)
            .expect("handle request")
            .expect("response");
        let resources = response
            .get("result")
            .and_then(|r| r.get("resources"))
            .and_then(Value::as_array)
            .expect("resources array");
        assert!(
            resources
                .iter()
                .any(|r| r.get("uri") == Some(&json!("mem://facts")))
        );
        assert!(
            resources
                .iter()
                .any(|r| r.get("uri") == Some(&json!("mem://commands")))
        );
    }

    #[test]
    fn read_message_parses_content_length_frame() {
        let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}});
        let body = serde_json::to_string(&request).expect("serialize request");
        let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut reader = BufReader::new(Cursor::new(input.into_bytes()));

        let parsed = read_message(&mut reader)
            .expect("parse framed message")
            .expect("message");
        assert_eq!(parsed, request);
    }

    #[test]
    fn read_message_parses_raw_json_message() {
        let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}});
        let body = serde_json::to_string(&request).expect("serialize request");
        let mut reader = BufReader::new(Cursor::new(body.into_bytes()));

        let parsed = read_message(&mut reader)
            .expect("parse raw json message")
            .expect("message");
        assert_eq!(parsed, request);
    }

    #[test]
    fn write_message_emits_newline_delimited_json() {
        let message = json!({"jsonrpc":"2.0","id":1,"result":{}});
        let mut buffer = Vec::new();
        write_message(&mut buffer, &message).expect("write message");

        let output = String::from_utf8(buffer).expect("utf8 output");
        assert!(
            !output.contains("Content-Length"),
            "output must not use LSP framing"
        );
        assert!(output.ends_with('\n'), "message must be newline-delimited");
        let parsed: Value = serde_json::from_str(output.trim_end()).expect("parse written message");
        assert_eq!(parsed, message);
    }
}
