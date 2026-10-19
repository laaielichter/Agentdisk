//! MCP server over stdio (newline-delimited JSON-RPC 2.0).
//!
//! Tools are read-only, except `agentdisk_plan`, which writes a plan file.
//! There is deliberately **no** tool to apply, restore, purge or compact:
//! an AI agent can never approve its own deletion. A human runs
//! `agentdisk apply <id>` in a terminal and types a code.

use crate::advise;
use crate::check;
use crate::cleanup::{self, PlanOptions};
use crate::config;
use crate::explain;
use crate::json::{self, Value};
use crate::scan::{self, ScanOptions};
use crate::size::{self, human};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

const PROTOCOLS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

fn projects_schema() -> Value {
    Value::obj(vec![
        ("type", "array".into()),
        ("items", Value::obj(vec![("type", "string".into())])),
        (
            "description",
            "Folders with repositories to scan. Defaults to `projects` in agentdisk's config.toml."
                .into(),
        ),
    ])
}

fn tool(name: &str, title: &str, desc: &str, props: Vec<(&str, Value)>, read_only: bool) -> Value {
    Value::obj(vec![
        ("name", name.into()),
        ("title", title.into()),
        ("description", desc.into()),
        (
            "inputSchema",
            Value::obj(vec![
                ("type", "object".into()),
                ("properties", Value::obj(props)),
                ("additionalProperties", false.into()),
            ]),
        ),
        (
            "annotations",
            Value::obj(vec![
                ("readOnlyHint", read_only.into()),
                ("destructiveHint", false.into()),
                ("idempotentHint", true.into()),
                ("openWorldHint", false.into()),
            ]),
        ),
    ])
}

pub fn tools() -> Value {
    let num = |d: &str| {
        Value::obj(vec![
            ("type", "integer".into()),
            ("minimum", 0u64.into()),
            ("description", d.into()),
        ])
    };
    Value::Arr(vec![
        tool(
            "agentdisk_scan",
            "Scan agent disk use",
            "Show how much disk space AI coding agents use: agent data (transcripts, checkpoints, caches), git worktrees, and dependency/build folders. Read-only.",
            vec![("projects", projects_schema()), ("json", Value::obj(vec![("type", "boolean".into())]))],
            true,
        ),
        tool(
            "agentdisk_size",
            "Measure a folder",
            "Measure one folder: logical size, size on disk, and how much deleting it would really free (hardlinks counted once). Read-only.",
            vec![("path", Value::obj(vec![("type", "string".into())]))],
            true,
        ),
        tool(
            "agentdisk_explain",
            "Explain a folder",
            "Say what a folder is (agent data, git worktree, build folder), which agent or tool made it, and whether a cleanup plan would include it and why. Read-only.",
            vec![("path", Value::obj(vec![("type", "string".into())]))],
            true,
        ),
        tool(
            "agentdisk_advise",
            "Disk saving advice",
            "Suggest ways to need less space: shared package stores (pnpm, uv), duplicate dependency trees across worktrees, cache cleanup commands, agent settings. Read-only; runs nothing.",
            vec![("projects", projects_schema())],
            true,
        ),
        tool(
            "agentdisk_plan",
            "Make a cleanup plan",
            "Make a cleanup plan and save it. Nothing is moved or deleted. Only a human can apply it, in a terminal, with `agentdisk apply <id>` and a confirmation code. Tell the user the command; never try to run it yourself.",
            vec![
                ("projects", projects_schema()),
                ("worktree_days", num("Minimum idle days for worktrees (default 7).")),
                ("build_days", num("Minimum idle days for build folders (default 14).")),
            ],
            false,
        ),
        tool(
            "agentdisk_check",
            "Quick disk check",
            "Fast check of free space and number of agent worktrees. Use it before starting work that creates worktrees or large builds.",
            vec![],
            true,
        ),
        tool(
            "agentdisk_quarantine",
            "List quarantine",
            "List items moved to quarantine by earlier cleanups, with their plan ids. Read-only.",
            vec![],
            true,
        ),
    ])
}

fn projects_arg(args: &Value, cfg: &config::Config) -> Vec<PathBuf> {
    match args.get("projects").and_then(Value::as_arr) {
        Some(list) => list
            .iter()
            .filter_map(|v| v.as_str().map(PathBuf::from))
            .collect(),
        None => cfg.projects.clone(),
    }
}

/// Run one tool. `Err` = tool error shown to the model.
pub fn call(name: &str, args: &Value) -> Result<String, String> {
    let cfg = config::load();
    let scan_opts = |args: &Value| ScanOptions {
        projects: projects_arg(args, &cfg),
        adapter_dirs: cfg.adapters.clone(),
        max_depth: cfg.max_depth,
        ..ScanOptions::default()
    };
    match name {
        "agentdisk_scan" => {
            let r = scan::run(&scan_opts(args));
            Ok(if args.get("json").and_then(Value::as_bool) == Some(true) {
                scan::to_json(&r).pretty()
            } else {
                scan::render(&r)
            })
        }
        "agentdisk_size" => {
            let p = args
                .get("path")
                .and_then(Value::as_str)
                .ok_or("missing 'path'")?;
            let t = size::measure(std::path::Path::new(p), size::default_threads())
                .map_err(|e| format!("{p}: {e}"))?;
            Ok(format!(
                "{p}\nfiles: {}  folders: {}\nsize: {}\non disk: {}\nfreed if deleted: {}\n",
                t.files,
                t.dirs,
                human(t.logical_bytes),
                human(t.allocated_bytes),
                human(t.reclaimable_bytes)
            ))
        }
        "agentdisk_explain" => {
            let p = args
                .get("path")
                .and_then(Value::as_str)
                .ok_or("missing 'path'")?;
            explain::explain(std::path::Path::new(p)).map(|e| explain::render(&e))
        }
        "agentdisk_advise" => {
            let r = scan::run(&scan_opts(args));
            Ok(advise::render(&advise::advise(&r)))
        }
        "agentdisk_plan" => {
            let opts = PlanOptions {
                scan: scan_opts(args),
                worktree_days: args
                    .get("worktree_days")
                    .and_then(Value::as_u64)
                    .unwrap_or(cfg.worktree_days),
                build_days: args
                    .get("build_days")
                    .and_then(Value::as_u64)
                    .unwrap_or(cfg.build_days),
                ..PlanOptions::default()
            };
            let (plan, _) = cleanup::plan_now(&opts);
            let path = cleanup::save(&plan).map_err(|e| format!("could not save plan: {e}"))?;
            Ok(cleanup::render_plan(&plan, Some(&path))
                + "\nNOTE FOR THE AGENT: you cannot apply this plan. Show the user the command above; they run it themselves.\n")
        }
        "agentdisk_check" => Ok(check::render(&check::run(20, 20))),
        "agentdisk_quarantine" => Ok(cleanup::render_quarantine()),
        other => Err(format!("unknown tool '{other}'")),
    }
}

fn response(id: &Value, result: Value) -> Value {
    Value::obj(vec![
        ("jsonrpc", "2.0".into()),
        ("id", id.clone()),
        ("result", result),
    ])
}

fn error(id: &Value, code: i64, msg: &str) -> Value {
    Value::obj(vec![
        ("jsonrpc", "2.0".into()),
        ("id", id.clone()),
        (
            "error",
            Value::obj(vec![
                ("code", (code as f64).into()),
                ("message", msg.into()),
            ]),
        ),
    ])
}

/// Handle one message. `None` for notifications.
pub fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::obj(vec![]));
    Some(match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("");
            let version = if PROTOCOLS.contains(&asked) {
                asked
            } else {
                PROTOCOLS[1]
            };
            response(
                &id,
                Value::obj(vec![
                    ("protocolVersion", version.into()),
                    ("capabilities", Value::obj(vec![("tools", Value::obj(vec![]))])),
                    (
                        "serverInfo",
                        Value::obj(vec![
                            ("name", "agentdisk".into()),
                            ("title", "agentdisk".into()),
                            ("version", crate::VERSION.into()),
                        ]),
                    ),
                    (
                        "instructions",
                        "agentdisk reports disk space used by AI coding agents. Call agentdisk_check before creating many worktrees or big builds. You may make a cleanup plan, but only the human can apply it."
                            .into(),
                    ),
                ]),
            )
        }
        "ping" => response(&id, Value::obj(vec![])),
        "tools/list" => response(&id, Value::obj(vec![("tools", tools())])),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or(Value::obj(vec![]));
            if !tools()
                .as_arr()
                .unwrap_or(&[])
                .iter()
                .any(|t| t.get("name").and_then(Value::as_str) == Some(name))
            {
                return Some(error(&id, -32602, &format!("unknown tool '{name}'")));
            }
            let (text, is_error) = match call(name, &args) {
                Ok(t) => (t, false),
                Err(e) => (e, true),
            };
            response(
                &id,
                Value::obj(vec![
                    (
                        "content",
                        Value::Arr(vec![Value::obj(vec![
                            ("type", "text".into()),
                            ("text", text.into()),
                        ])]),
                    ),
                    ("isError", is_error.into()),
                ]),
            )
        }
        _ => error(&id, -32601, &format!("method not found: {method}")),
    })
}

/// Serve on stdin/stdout until stdin closes. Logs go to stderr only.
pub fn serve() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match json::parse(&line) {
            Ok(Value::Arr(batch)) => {
                let replies: Vec<Value> = batch.iter().filter_map(handle).collect();
                (!replies.is_empty()).then_some(Value::Arr(replies))
            }
            Ok(msg) => handle(&msg),
            Err(e) => Some(error(&Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = reply {
            writeln!(stdout, "{r}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_and_list() {
        let init = json::parse(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#).unwrap();
        let r = handle(&init).unwrap();
        assert_eq!(
            r.get("result")
                .unwrap()
                .get("protocolVersion")
                .unwrap()
                .as_str(),
            Some("2025-06-18")
        );
        let notif =
            json::parse(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap();
        assert!(handle(&notif).is_none());
        let list = json::parse(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        let r = handle(&list).unwrap();
        let names: Vec<String> = r
            .get("result")
            .unwrap()
            .get("tools")
            .unwrap()
            .as_arr()
            .unwrap()
            .iter()
            .map(|t| t.get("name").unwrap().as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"agentdisk_scan".to_string()));
        for forbidden in ["apply", "restore", "purge", "compact", "delete"] {
            assert!(
                names.iter().all(|n| !n.contains(forbidden)),
                "MCP must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn unknown_tool_and_method() {
        let m = json::parse(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"agentdisk_apply"}}"#,
        )
        .unwrap();
        assert!(handle(&m).unwrap().get("error").is_some());
        let m = json::parse(r#"{"jsonrpc":"2.0","id":4,"method":"nope"}"#).unwrap();
        assert!(handle(&m).unwrap().get("error").is_some());
    }

    #[test]
    fn size_tool_reports_error_for_missing_path() {
        let m = json::parse(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"agentdisk_size","arguments":{"path":"/no/such/agentdisk"}}}"#).unwrap();
        let r = handle(&m).unwrap();
        assert_eq!(
            r.get("result").unwrap().get("isError").unwrap().as_bool(),
            Some(true)
        );
    }
}
