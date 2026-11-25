//! The read-only commands of `viv`: `status`, `doctor`, `config list`, `sessions`, `session <id>` and `profiles`.
//! Each builds one snapshot (a JSON value) and renders both the text and `--json` output from it (R5.2); results go
//! to stdout, problems to stderr. A usage error exits 2.

use serde_json::{Value, json};
use vivianne_runtime::{config, profiles, session, store, thread_refs};

/// A usage error: printed, and the process exits 2.
pub struct Usage(pub String);

fn take_switch(args: &mut Vec<String>, flag: &str) -> bool {
    args.iter()
        .position(|a| a == flag)
        .map(|at| args.remove(at))
        .is_some()
}

fn take_value(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, Usage> {
    let Some(at) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    let value = args
        .get(at + 1)
        .cloned()
        .ok_or_else(|| Usage(format!("{flag} needs a value")))?;
    args.drain(at..=at + 1);
    Ok(Some(value))
}

fn print(json_out: bool, snapshot: &Value, text: impl FnOnce(&Value) -> String) {
    if json_out {
        println!("{snapshot}");
    } else {
        print!("{}", text(snapshot));
    }
}

fn workspace() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// `viv status [--json]`
pub fn status(mut args: Vec<String>) -> Result<(), Usage> {
    let json_out = take_switch(&mut args, "--json");
    if let Some(extra) = args.first() {
        return Err(Usage(format!("status takes no argument, got `{extra}`")));
    }
    let catalog = vivianne_remote::catalog::Catalog::load();
    let snapshot = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "home": vivianne_ai::paths::viv_home().map(|p| p.display().to_string()).unwrap_or_default(),
        "workspace": workspace().display().to_string(),
        "threads": store::list_everywhere().len(),
        "machines": catalog.as_ref().map(|c| c.ssh.len()).unwrap_or(0),
        "machines_file_ok": catalog.is_ok(),
    });
    print(json_out, &snapshot, |s| {
        format!(
            "viv {}\nhome       {}\nworkspace  {}\nthreads    {}\nmachines   {}\n",
            s["version"].as_str().unwrap_or_default(),
            s["home"].as_str().unwrap_or_default(),
            s["workspace"].as_str().unwrap_or_default(),
            s["threads"],
            s["machines"]
        )
    });
    Ok(())
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> Value {
    json!({"name": name, "ok": ok, "detail": detail.into()})
}

/// `viv doctor [--json]`: what is wrong, if anything. Exits 1 when a check fails.
pub fn doctor(mut args: Vec<String>) -> Result<bool, Usage> {
    let json_out = take_switch(&mut args, "--json");
    if let Some(extra) = args.first() {
        return Err(Usage(format!("doctor takes no argument, got `{extra}`")));
    }
    let ws = workspace();
    let mut checks = Vec::new();
    match vivianne_ai::paths::viv_home() {
        Ok(home) => {
            let probe = home.join(".doctor");
            let writable = std::fs::create_dir_all(&home)
                .and_then(|()| std::fs::write(&probe, b"x"))
                .is_ok();
            // The probe is ours; a failure to remove it only leaves a small file.
            let _ = std::fs::remove_file(&probe);
            checks.push(check(
                "home is writable",
                writable,
                home.display().to_string(),
            ));
        }
        Err(why) => checks.push(check("home is writable", false, why)),
    }
    checks.push(match vivianne_remote::catalog::Catalog::load() {
        Ok(c) => check("machines file", true, format!("{} saved", c.ssh.len())),
        Err(why) => check("machines file", false, why),
    });
    let project = config::load_project(&ws);
    checks.push(check(
        ".viv.json",
        project.warnings.is_empty(),
        project.warnings.join("; "),
    ));
    checks.push(match vivianne_providers::env::selection() {
        Ok(s) => check(
            "model",
            vivianne_providers::env::key_for(&s.provider).is_some(),
            format!("{} / {}", s.provider, s.model),
        ),
        Err(why) => check("model", false, why),
    });
    let (_, problems) = profiles::list(&ws);
    checks.push(check(
        "agent profiles",
        problems.is_empty(),
        problems.join("; "),
    ));
    checks.push(check(
        "ssh",
        which_ssh(),
        "needed only to reach other machines",
    ));
    let all_ok = checks.iter().all(|c| c["ok"] == true);
    let snapshot = json!({"ok": all_ok, "checks": checks});
    print(json_out, &snapshot, |s| {
        let mut out = String::new();
        for c in s["checks"].as_array().into_iter().flatten() {
            let mark = if c["ok"] == true { "ok  " } else { "FAIL" };
            out.push_str(&format!(
                "{mark} {}  {}\n",
                c["name"].as_str().unwrap_or_default(),
                c["detail"].as_str().unwrap_or_default()
            ));
        }
        out
    });
    Ok(all_ok)
}

fn which_ssh() -> bool {
    std::process::Command::new(vivianne_remote::ssh::program())
        .arg("-V")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// `viv config list [--show-origin] [--json]`
pub fn config_cmd(mut args: Vec<String>) -> Result<(), Usage> {
    let json_out = take_switch(&mut args, "--json");
    let origin = take_switch(&mut args, "--show-origin");
    match args.first().map(String::as_str) {
        Some("list") => {}
        other => {
            return Err(Usage(format!(
                "config needs `list`, got `{}`",
                other.unwrap_or("nothing")
            )));
        }
    }
    let resolved = config::resolve(&workspace(), None);
    for warning in &resolved.warnings {
        eprintln!("viv: {warning}");
    }
    let rows: Vec<Value> = config::list(&resolved)
        .into_iter()
        .map(|(k, v, o)| json!({"key": k, "value": v, "origin": o}))
        .collect();
    let snapshot = json!({"settings": rows});
    print(json_out, &snapshot, |s| {
        let mut out = String::new();
        for r in s["settings"].as_array().into_iter().flatten() {
            let (k, v) = (
                r["key"].as_str().unwrap_or_default(),
                r["value"].as_str().unwrap_or_default(),
            );
            if origin {
                out.push_str(&format!(
                    "{k} = {v}    ({})\n",
                    r["origin"].as_str().unwrap_or_default()
                ));
            } else {
                out.push_str(&format!("{k} = {v}\n"));
            }
        }
        out
    });
    Ok(())
}

/// `viv sessions [--limit N] [--json]`: saved threads, newest first.
pub fn sessions(mut args: Vec<String>) -> Result<(), Usage> {
    let json_out = take_switch(&mut args, "--json");
    let limit: usize = take_value(&mut args, "--limit")?.map_or(Ok(20), |v| {
        v.parse()
            .map_err(|_| Usage("--limit needs a number".to_owned()))
    })?;
    if let Some(extra) = args.first() {
        return Err(Usage(format!("sessions takes no argument, got `{extra}`")));
    }
    let all = store::list_everywhere();
    let total = all.len();
    let rows: Vec<Value> = all
        .into_iter()
        .take(limit)
        .map(|(workspace, s)| json!({"id": s.id, "title": s.title, "workspace": workspace, "updated_at": s.updated_at}))
        .collect();
    let snapshot = json!({"total": total, "sessions": rows});
    print(json_out, &snapshot, |s| {
        let mut out = String::new();
        for r in s["sessions"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "{}  {}  {}\n",
                r["id"].as_str().unwrap_or_default(),
                r["title"].as_str().unwrap_or("(untitled)"),
                r["workspace"].as_str().unwrap_or_default()
            ));
        }
        if total == 0 {
            out.push_str("No saved threads.\n");
        }
        out
    });
    Ok(())
}

/// `viv session <id> [--tail N] [--json]`: one saved thread.
pub fn session(mut args: Vec<String>) -> Result<(), Usage> {
    let json_out = take_switch(&mut args, "--json");
    let tail: Option<usize> = take_value(&mut args, "--tail")?
        .map(|v| {
            v.parse()
                .map_err(|_| Usage("--tail needs a number".to_owned()))
        })
        .transpose()?;
    let id = args
        .first()
        .cloned()
        .ok_or_else(|| Usage("session needs a thread id".to_owned()))?;
    let Some(thread) = store::load(&id) else {
        eprintln!("viv: no saved thread has the id {id}");
        std::process::exit(1);
    };
    let skip = tail.map_or(0, |n| thread.history.len().saturating_sub(n));
    let messages: Vec<Value> = thread
        .history
        .iter()
        .skip(skip)
        .map(|m| json!({"role": format!("{:?}", m.role).to_lowercase(), "text": m.text}))
        .collect();
    let snapshot = json!({"id": thread.id, "title": thread.title, "workspace": thread.workspace, "model": thread.model, "messages": messages});
    print(json_out, &snapshot, |s| {
        let mut out = format!(
            "{}  ({})\n\n",
            s["title"].as_str().unwrap_or_default(),
            s["model"].as_str().unwrap_or_default()
        );
        for m in s["messages"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "[{}] {}\n\n",
                m["role"].as_str().unwrap_or_default(),
                m["text"].as_str().unwrap_or_default()
            ));
        }
        out
    });
    Ok(())
}

/// `viv profiles [--json]`
pub fn profiles_cmd(mut args: Vec<String>) -> Result<(), Usage> {
    let json_out = take_switch(&mut args, "--json");
    if let Some(extra) = args.first() {
        return Err(Usage(format!("profiles takes no argument, got `{extra}`")));
    }
    let (all, problems) = profiles::list(&workspace());
    for p in &problems {
        eprintln!("viv: {p}");
    }
    let rows: Vec<Value> = all
        .iter()
        .map(|p| json!({"name": p.name, "description": p.description, "source": format!("{:?}", p.source).to_lowercase()}))
        .collect();
    let snapshot = json!({"profiles": rows});
    print(json_out, &snapshot, |s| {
        let mut out = String::new();
        for r in s["profiles"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "{:<14} {:<10} {}\n",
                r["name"].as_str().unwrap_or_default(),
                r["source"].as_str().unwrap_or_default(),
                r["description"].as_str().unwrap_or_default()
            ));
        }
        out
    });
    Ok(())
}

/// `viv thread list|events|fork|mention`: this project's saved conversations as numbered events, branched at a turn, or
/// named in a message.
///
/// - `viv thread list [--json]`
/// - `viv thread events <id> [--from N] [--to M] [--json]`
/// - `viv thread fork <id> --before <turn> [--json]`
/// - `viv thread mention <id>`
pub fn thread_cmd(mut args: Vec<String>) -> Result<(), Usage> {
    let json_out = take_switch(&mut args, "--json");
    let from = take_value(&mut args, "--from")?;
    let to = take_value(&mut args, "--to")?;
    let before = take_value(&mut args, "--before")?;
    let number = |what: &str, text: Option<String>| -> Result<Option<usize>, Usage> {
        text.map(|t| t.parse::<usize>().map_err(|_| Usage(format!("{what} needs a whole number, got `{t}`")))).transpose()
    };
    let (from, to, before) = (number("--from", from)?, number("--to", to)?, number("--before", before)?);
    let folder = workspace().display().to_string();
    let id = |args: &[String]| args.get(1).cloned().ok_or_else(|| Usage("this thread command needs a thread id".to_owned()));
    match args.first().map(String::as_str) {
        Some("list") => {
            let rows: Vec<Value> = store::list(&folder).into_iter().map(|t| json!({"id": t.id, "title": t.title, "updated_at": t.updated_at})).collect();
            print(json_out, &json!({"threads": rows}), |s| {
                s["threads"].as_array().into_iter().flatten().map(|t| format!("{}  {}\n", t["id"].as_str().unwrap_or_default(), t["title"].as_str().unwrap_or_default())).collect()
            });
            Ok(())
        }
        Some("events") => {
            let id = id(&args)?;
            let thread = store::load(&id).filter(|t| t.workspace == folder).ok_or_else(|| Usage(format!("there is no saved thread {id} in this project")))?;
            let events = session::range(&thread.history, from.unwrap_or(0), to.unwrap_or(usize::MAX));
            let rows = serde_json::to_value(&events).unwrap_or(Value::Null);
            print(json_out, &json!({"turns": session::turn_count(&thread.history), "events": rows}), |s| {
                s["events"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|e| {
                        let text = e["text"].as_str().unwrap_or_default().replace('\n', " ");
                        let shown: String = text.chars().take(80).collect();
                        format!("{:>4}  turn {}  {}  {shown}\n", e["seq"], e["turn"], e["type"].as_str().unwrap_or_default())
                    })
                    .collect()
            });
            Ok(())
        }
        Some("fork") => {
            let id = id(&args)?;
            let before = before.ok_or_else(|| Usage("fork needs --before <turn>".to_owned()))?;
            store::load(&id).filter(|t| t.workspace == folder).ok_or_else(|| Usage(format!("there is no saved thread {id} in this project")))?;
            let forked = store::fork(&id, before).map_err(Usage)?;
            print(json_out, &json!({"id": forked.id, "title": forked.title, "messages": forked.history.len()}), |s| format!("{}\n", s["id"].as_str().unwrap_or_default()));
            Ok(())
        }
        Some("mention") => {
            let id = id(&args)?;
            let mention = thread_refs::mention_of(&id, &folder).ok_or_else(|| Usage(format!("there is no saved thread {id} in this project")))?;
            print(json_out, &json!({"mention": mention}), |s| format!("{}\n", s["mention"].as_str().unwrap_or_default()));
            Ok(())
        }
        other => Err(Usage(format!("thread needs `list`, `events`, `fork` or `mention`, got `{}`", other.unwrap_or("nothing")))),
    }
}
