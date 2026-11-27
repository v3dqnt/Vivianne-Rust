//! `viv thread search|trace|event|export`: find, relate and export saved conversations. The logic is in
//! `vivianne_runtime::query` and `export_bundle`; this only reads flags and prints.

use serde_json::{Value, json};
use vivianne_runtime::export::{self, Options};
use vivianne_runtime::export_bundle;
use vivianne_runtime::query::filters::{EventFilter, Kind, ThreadFilter};
use vivianne_runtime::query::search::{Index, Search};
use vivianne_runtime::query::{Query, QueryError};

use crate::inspect::{Usage, print, take_switch, take_value};

fn usage(e: &QueryError) -> Usage {
    Usage(e.to_string())
}

fn number(what: &str, text: Option<String>) -> Result<Option<usize>, Usage> {
    text.map(|t| {
        t.parse::<usize>()
            .map_err(|_| Usage(format!("{what} needs a whole number, got `{t}`")))
    })
    .transpose()
}

/// This project's thread, or the reason there is none.
fn mine(query: &Query, id: &str, folder: &str) -> Result<(), Usage> {
    let thread = query.read_thread(id).map_err(|e| usage(&e))?;
    if thread.workspace == folder {
        Ok(())
    } else {
        Err(Usage(format!(
            "there is no saved thread {id} in this project"
        )))
    }
}

pub fn run(sub: &str, mut args: Vec<String>, json_out: bool, folder: &str) -> Result<(), Usage> {
    let query = Query::standard().map_err(|e| usage(&e))?;
    match sub {
        "search" => search(&query, &mut args, json_out, folder),
        "trace" => trace(&query, &mut args, json_out, folder),
        "event" => event(&query, &mut args, json_out, folder),
        "export" => export_cmd(&query, &mut args, folder),
        other => Err(Usage(format!("no such thread command `{other}`"))),
    }
}

fn search(
    query: &Query,
    args: &mut Vec<String>,
    json_out: bool,
    folder: &str,
) -> Result<(), Usage> {
    let thread = take_value(args, "--thread")?;
    let limit = number("--limit", take_value(args, "--limit")?)?;
    let cursor = take_value(args, "--cursor")?;
    let kind = take_value(args, "--kind")?;
    let everywhere = take_switch(args, "--all-projects");
    let words = args.get(1..).unwrap_or_default().join(" ");
    let mut events = Vec::new();
    if let Some(kind) = kind {
        let kind = Kind::parse(&kind).ok_or_else(|| {
            Usage(format!(
                "--kind is user, context, assistant, tool-result or checkpoint, not `{kind}`"
            ))
        })?;
        events.push(EventFilter::Kind(vec![kind]));
    }
    let threads = if everywhere {
        Vec::new()
    } else {
        vec![ThreadFilter::Workspace(vec![folder.to_owned()])]
    };
    let request = Search {
        query: words,
        threads,
        events,
        limit,
        cursor,
    };
    let path = vivianne_ai::paths::viv_agent_dir()
        .map_err(Usage)?
        .join("search.db");
    let mut index = Index::open(&path).map_err(|e| usage(&e))?;
    index.reconcile(query).map_err(|e| usage(&e))?;
    let snapshot = if let Some(id) = thread {
        if !everywhere {
            mine(query, &id, folder)?;
        }
        let page = index.search_events(&id, &request).map_err(|e| usage(&e))?;
        json!({ "hits": page.items, "next_cursor": page.next_cursor })
    } else {
        let page = index.search_threads(&request).map_err(|e| usage(&e))?;
        json!({ "hits": page.items, "next_cursor": page.next_cursor })
    };
    print(json_out, &snapshot, |s| {
        let mut out: String = s["hits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|h| {
                let best = if h.get("best").is_some() {
                    &h["best"]
                } else {
                    h
                };
                let id = h["thread"]
                    .as_str()
                    .or_else(|| best["thread"].as_str())
                    .unwrap_or_default();
                format!(
                    "{id}  #{} {}  {}\n",
                    best["seq"],
                    best["kind"].as_str().unwrap_or_default(),
                    best["snippet"]
                        .as_str()
                        .unwrap_or_default()
                        .replace('\n', " ")
                )
            })
            .collect();
        if let Some(next) = s["next_cursor"].as_str() {
            out.push_str(&format!("more: --cursor {next}\n"));
        }
        out
    });
    Ok(())
}

fn trace(query: &Query, args: &mut Vec<String>, json_out: bool, folder: &str) -> Result<(), Usage> {
    let seq = number("--seq", take_value(args, "--seq")?)?;
    let id = args
        .get(1)
        .cloned()
        .ok_or_else(|| Usage("trace needs a thread id".to_owned()))?;
    mine(query, &id, folder)?;
    let snapshot = match seq {
        Some(seq) => serde_json::to_value(query.trace_event(&id, seq).map_err(|e| usage(&e))?),
        None => serde_json::to_value(query.trace_thread(&id).map_err(|e| usage(&e))?),
    }
    .unwrap_or(Value::Null);
    print(json_out, &snapshot, |s| {
        format!("{}\n", serde_json::to_string_pretty(s).unwrap_or_default())
    });
    Ok(())
}

fn event(query: &Query, args: &mut Vec<String>, json_out: bool, folder: &str) -> Result<(), Usage> {
    let before = number("--before", take_value(args, "--before")?)?.unwrap_or(0);
    let after = number("--after", take_value(args, "--after")?)?.unwrap_or(0);
    let id = args
        .get(1)
        .cloned()
        .ok_or_else(|| Usage("event needs a thread id and a position".to_owned()))?;
    let seq = number("the position", args.get(2).cloned())?
        .ok_or_else(|| Usage("event needs a thread id and a position".to_owned()))?;
    mine(query, &id, folder)?;
    let window = query
        .read_event(&id, seq, before, after)
        .map_err(|e| usage(&e))?;
    let snapshot = json!({
        "thread": window.thread, "generation": window.generation, "start_seq": window.start_seq, "end_seq": window.end_seq,
        "target": window.target, "events": window.events,
    });
    print(json_out, &snapshot, |s| {
        s["events"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, m)| {
                format!(
                    "{:>4}  {}  {}\n",
                    s["start_seq"].as_u64().unwrap_or(0) as usize + i,
                    m["role"].as_str().unwrap_or_default(),
                    m["text"].as_str().unwrap_or_default().replace('\n', " ")
                )
            })
            .collect()
    });
    Ok(())
}

fn export_cmd(query: &Query, args: &mut Vec<String>, folder: &str) -> Result<(), Usage> {
    let out_file = take_value(args, "--out")?;
    let format = take_value(args, "--format")?.unwrap_or_else(|| "jsonl".to_owned());
    let after = number("--after", take_value(args, "--after")?)?.unwrap_or(0);
    let raw = take_switch(args, "--raw");
    let id = args
        .get(1)
        .cloned()
        .ok_or_else(|| Usage("export needs a thread id".to_owned()))?;
    mine(query, &id, folder)?;
    let thread = query.read_thread(&id).map_err(|e| usage(&e))?;
    let options = Options {
        redact: !raw,
        after,
    };
    let bytes: Vec<u8> = match format.as_str() {
        "jsonl" => {
            let mut out = Vec::new();
            export::write_to(&mut out, &thread, options).map_err(Usage)?;
            out
        }
        "markdown" => export_bundle::markdown(&thread, options).into_bytes(),
        "zip" => {
            if out_file.is_none() {
                return Err(Usage(
                    "--format zip needs --out FILE (an archive is not printed)".to_owned(),
                ));
            }
            let read = export_bundle::standard_reader();
            export_bundle::bundle(query, &id, options, &read)
                .map_err(|e| Usage(e.to_string()))?
                .0
        }
        other => {
            return Err(Usage(format!(
                "--format is jsonl, markdown or zip, not `{other}`"
            )));
        }
    };
    match out_file {
        Some(file) => {
            std::fs::write(&file, &bytes).map_err(|e| Usage(format!("cannot write {file}: {e}")))?
        }
        None => {
            use std::io::Write as _;
            std::io::stdout()
                .lock()
                .write_all(&bytes)
                .map_err(|e| Usage(e.to_string()))?;
        }
    }
    eprintln!(
        "{} bytes{}",
        bytes.len(),
        if raw { " (not redacted)" } else { "" }
    );
    Ok(())
}
