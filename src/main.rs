use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

use noroshi::alert::{Alert, session_label};
use noroshi::config::{Config, Policy};
use noroshi::sink::{self, Ureq};

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Parser)]
#[command(name = "noroshi", about = "Signal-fire alerts for AI coding agents")]
struct Cli {
    /// Config file; defaults to `$XDG_CONFIG_HOME/noroshi/noroshi.yaml`.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Claude Code hook: read the payload on stdin and signal the event if its policy says so. Always exits 0.
    Hook,
    /// Report the config and whether each backend can deliver, without sending anything.
    Check,
    /// Send one test alert through every configured backend.
    Test {
        /// Event whose priority, tags and title the test uses.
        #[arg(long, default_value = "Stop")]
        event: String,
    },
}

fn config_path(flag: Option<PathBuf>) -> PathBuf {
    flag.unwrap_or_else(|| {
        okiba::Okiba::for_app("noroshi")
            .path(okiba::Tier::Config, "noroshi.yaml")
            .into_path_buf()
    })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let path = config_path(cli.config);
    match cli.command {
        Command::Hook => {
            hook(&path);
            ExitCode::SUCCESS
        }
        Command::Check => check(&path),
        Command::Test { event } => test(&path, &event),
    }
}

fn hook(path: &std::path::Path) {
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return;
    }
    let config = match Config::load(path) {
        Ok(Some(c)) => c,
        Ok(None) => return,
        Err(e) => {
            eprintln!("noroshi: {}: {e}", path.display());
            return;
        }
    };
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let Some(alert) = Alert::from_payload(&payload, &config.host) else {
        return;
    };
    if config.policy(&alert.event) == Policy::Silent {
        return;
    }
    let transport = Ureq::new(TIMEOUT);
    for s in sink::sinks(&config) {
        if let Err(e) = sink::deliver(s.as_ref(), &alert, &transport) {
            eprintln!("noroshi: {}: {e}", s.name());
        }
    }
}

fn load_or_report(path: &std::path::Path) -> Option<Config> {
    match Config::load(path) {
        Ok(Some(c)) => Some(c),
        Ok(None) => {
            println!(
                "config: {} does not exist; nothing is signalled",
                path.display()
            );
            None
        }
        Err(e) => {
            println!("config: {}: {e}", path.display());
            None
        }
    }
}

fn check(path: &std::path::Path) -> ExitCode {
    let Some(config) = load_or_report(path) else {
        return ExitCode::FAILURE;
    };
    println!("config: {}", path.display());
    println!(
        "host: {}",
        if config.host.is_empty() {
            "-"
        } else {
            &config.host
        }
    );
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

fn test(path: &std::path::Path, event: &str) -> ExitCode {
    let Some(config) = load_or_report(path) else {
        return ExitCode::FAILURE;
    };
    let alert = Alert {
        event: event.to_string(),
        host: config.host.clone(),
        session: "test".into(),
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
