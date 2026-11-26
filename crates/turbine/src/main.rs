//! Turbine: a retro DOS-style, AI-first code editor and IDE.

mod agent;
mod server;
mod workbench;

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use turbine_ui::{App, Event, Surface};
use workbench::{Request, Workbench};

const USAGE: &str = "\
turbine [options] [files...]

Options:
  --automation [PATH]   Listen for agent commands (JSON lines) on a Unix socket.
                        Default: $XDG_RUNTIME_DIR/turbine-<pid>.sock
  --speed CPS           Agent typing speed in characters per second (0 = instant)
  --demo                Start the agent demo right away
  --headless            Run without a terminal (for tests and screenshots)
  --size WxH            Screen size for --headless (default 100x30)
  --script FILE         Run JSON-line agent commands from FILE (headless or not)
  --screenshot FILE     Write the final screen as text to FILE (headless)
  -h, --help            Show this help
";

struct Args {
    files: Vec<PathBuf>,
    automation: Option<Option<PathBuf>>,
    speed: Option<f64>,
    demo: bool,
    headless: bool,
    size: (i32, i32),
    script: Option<PathBuf>,
    screenshot: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        files: Vec::new(),
        automation: None,
        speed: None,
        demo: false,
        headless: false,
        size: (100, 30),
        script: None,
        screenshot: None,
    };
    let mut it = std::env::args().skip(1).peekable();
    while let Some(arg) = it.next() {
        let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--automation" => {
                let path = it.next_if(|v| !v.starts_with('-')).map(PathBuf::from);
                a.automation = Some(path);
            }
            "--speed" => a.speed = Some(val("--speed")?.parse().map_err(|_| "bad --speed")?),
            "--demo" => a.demo = true,
            "--headless" => a.headless = true,
            "--size" => {
                let v = val("--size")?;
                let (w, h) = v.split_once('x').ok_or("--size wants WxH")?;
                a.size = (w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?);
            }
            "--script" => a.script = Some(PathBuf::from(val("--script")?)),
            "--screenshot" => a.screenshot = Some(PathBuf::from(val("--screenshot")?)),
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            _ => a.files.push(PathBuf::from(arg)),
        }
    }
    Ok(a)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("turbine: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let mut wb = Workbench::new(&args.files);
    if let Some(s) = args.speed {
        wb.driver_mut().cps = s;
    } else if args.headless {
        wb.driver_mut().cps = 0.0;
    }

    let (tx, rx) = mpsc::channel::<Request>();
    wb.set_requests(rx);
    if let Some(path) = &args.automation {
        match server::listen(path.clone(), tx.clone()) {
            Ok(p) => eprintln!("turbine: automation socket {}", p.display()),
            Err(e) => eprintln!("turbine: automation socket failed: {e}"),
        }
    }
    if let Some(script) = &args.script {
        server::run_script(script.clone(), tx.clone());
    }
    if args.demo {
        wb.run_command("agent.demo");
    }

    if args.headless {
        wb.event(Event::Resize(args.size.0, args.size.1));
        let script_done = server::script_done();
        let start = Instant::now();
        loop {
            let next = wb.tick(Instant::now());
            let script_idle = args.script.is_none() || script_done.load(std::sync::atomic::Ordering::SeqCst);
            if wb.should_quit() || (script_idle && !wb.driver_mut().busy()) || start.elapsed() > Duration::from_secs(600) {
                break;
            }
            let wait = next.map(|t| t.saturating_duration_since(Instant::now())).unwrap_or(Duration::from_millis(5));
            std::thread::sleep(wait.min(Duration::from_millis(20)));
        }
        wb.tick(Instant::now());
        let mut surf = Surface::new(args.size.0, args.size.1);
        wb.render(&mut surf);
        let text = surf.to_text();
        match &args.screenshot {
            Some(p) => {
                if let Err(e) = std::fs::write(p, &text) {
                    eprintln!("turbine: can't write {}: {e}", p.display());
                    std::process::exit(1);
                }
            }
            None => print!("{text}"),
        }
        return;
    }

    if let Err(e) = turbine_tui::run(&mut wb) {
        eprintln!("turbine: {e}");
        std::process::exit(1);
    }
}
