use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

use noroshi::alert::{Alert, Context, Detail, session_label};
use noroshi::config::{Config, Policy};
use noroshi::sink::{self, Ureq};

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Parser)]
#[command(name = "noroshi", about = "Signal-fire alerts for AI coding agents")]
struct Cli {
    /// Replaces the user layer (`$XDG_CONFIG_HOME/noroshi/noroshi.yaml`); system and repo layers still merge.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Claude Code hook: read the payload on stdin and signal the event if its policy says so. Always exits 0.
    Hook,
    /// Report the merged config layers and whether each backend can deliver, without sending anything.
    Check,
    /// Send one test alert through every configured backend.
    Test {
        /// Event whose priority, tags and title the test uses.
        #[arg(long, default_value = "Stop")]
        event: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let explicit = cli.config.as_deref();
    match cli.command {
        Command::Hook => {
            hook(explicit);
            ExitCode::SUCCESS
        }
        Command::Check => check(explicit),
        Command::Test { event } => test(explicit, &event),
    }
}

fn hook(explicit: Option<&Path>) {
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return;
    }
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let cwd = payload
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .map(Path::new);
    let config = match Config::load(explicit, cwd) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("noroshi: {e}");
            return;
        }
    };
    let Some(alert) = Alert::from_payload(&payload, &config.host) else {
        return;
    };
    if config.policy(&alert.event) == Policy::Silent {
        return;
    }
    let alert = alert.with_detail(&payload, &config.render);
    let transport = Ureq::new(TIMEOUT);
    for s in sink::sinks(&config) {
        if let Err(e) = sink::deliver(s.as_ref(), &alert, &transport) {
            eprintln!("noroshi: {}: {e}", s.name());
        }
    }
}

fn load_or_report(explicit: Option<&Path>) -> Option<Config> {
    let cwd = std::env::current_dir().ok();
    let mut layers = Config::layers(cwd.as_deref());
    if let Some(p) = explicit {
        layers.push(p.to_path_buf());
    }
    if layers.is_empty() {
        println!("layers: none found; built-in defaults only");
    }
    for l in &layers {
        println!("layer: {}", l.display());
    }
    match Config::load(explicit, cwd.as_deref()) {
        Ok(c) => Some(c),
        Err(e) => {
            println!("config: {e}");
            None
        }
    }
}

fn check(explicit: Option<&Path>) -> ExitCode {
    let Some(config) = load_or_report(explicit) else {
        return ExitCode::FAILURE;
    };
    println!(
        "host: {}",
        if config.host.is_empty() {
            "-"
        } else {
            &config.host
        }
    );
    println!("render: {:?}", config.render.detail());
    for (event, policy) in &config.events {
        println!("event {event}: {policy}");
    }
    for p in config.problems() {
        println!("ignored: {p}");
    }
    let probe = Alert {
        event: "Stop".into(),
        host: config.host.clone(),
        session: session_label("check"),
        context: None,
    };
    let sinks = sink::sinks(&config);
    if sinks.is_empty() {
        println!("backends: none configured");
        return ExitCode::FAILURE;
    }
    let mut ready = 0;
    for s in &sinks {
        match s.request(&probe) {
            Ok(_) => {
                ready += 1;
                println!("backend {}: ready", s.name());
            }
            Err(e) => println!("backend {}: cannot deliver: {e}", s.name()),
        }
    }
    if ready == 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn test(explicit: Option<&Path>, event: &str) -> ExitCode {
    let Some(config) = load_or_report(explicit) else {
        return ExitCode::FAILURE;
    };
    let alert = Alert {
        event: event.to_string(),
        host: config.host.clone(),
        session: "test".into(),
        context: (config.render.detail() == Detail::Context).then(|| Context {
            project: Some("noroshi".into()),
            branch: Some("main".into()),
            kind: None,
            said: Some("This is a noroshi test alert: the real one quotes Claude here.".into()),
        }),
    };
    let sinks = sink::sinks(&config);
    if sinks.is_empty() {
        println!("backends: none configured");
        return ExitCode::FAILURE;
    }
    let transport = Ureq::new(TIMEOUT);
    let mut delivered = 0;
    for s in &sinks {
        match sink::deliver(s.as_ref(), &alert, &transport) {
            Ok(status) => {
                delivered += 1;
                println!("backend {}: sent (HTTP {status})", s.name());
            }
            Err(e) => println!("backend {}: {e}", s.name()),
        }
    }
    if delivered == 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
