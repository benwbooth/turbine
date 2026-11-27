//! Turbine: a retro DOS-style, AI-first code editor and IDE.

use turbine::{mcp, server, vim, workbench};

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use turbine_ui::{App, Event, Surface};
use workbench::{Request, Workbench};

const USAGE: &str = "\
turbine [options] [files...]
turbine mcp [--socket PATH]   MCP server (stdio) driving a running Turbine
turbine pack-extension CORE.wasm OUT.wasm
                              Make a WebAssembly extension component (see extensions/)

Options:
  --automation [PATH]   Agent commands (JSON lines) on this Unix socket (on by
                        default at $XDG_RUNTIME_DIR/turbine-<pid>.sock)
  --no-automation       No automation socket
  --keymap NAME         Editor keys: cua (default, DOS style) or vim
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
    no_automation: bool,
    speed: Option<f64>,
    keymap: Option<vim::Keymap>,
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
        no_automation: false,
        speed: None,
        keymap: None,
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
            "--no-automation" => a.no_automation = true,
            "--speed" => a.speed = Some(val("--speed")?.parse().map_err(|_| "bad --speed")?),
            "--keymap" => {
                let v = val("--keymap")?;
                a.keymap = Some(vim::Keymap::parse(&v).ok_or_else(|| format!("unknown keymap {v} (cua, vim)"))?);
            }
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
    // As a child trying a downloaded grammar: try it and exit.
    turbine_syntax::grammars::check_from_args();
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some("pack-extension") {
        // A core module built with wit-bindgen -> an extension component.
        let (Some(input), Some(output)) = (argv.get(2), argv.get(3)) else {
            eprintln!("usage: turbine pack-extension CORE.wasm OUT.wasm");
            std::process::exit(2);
        };
        let packed = std::fs::read(input).map_err(|e| e.to_string()).and_then(|b| turbine_ext::pack(&b).map_err(|e| format!("{e:#}")));
        match packed.and_then(|c| std::fs::write(output, c).map_err(|e| e.to_string())) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("turbine: {e}");
                std::process::exit(1);
            }
        }
    }
    if argv.get(1).map(String::as_str) == Some("mcp") {
        let socket = argv.iter().position(|a| a == "--socket").and_then(|i| argv.get(i + 1)).map(PathBuf::from);
        std::process::exit(mcp::run(socket));
    }
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("turbine: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let mut wb = Workbench::new(&args.files);
    // The chosen colours and style, kept between runs (not for headless tests).
    if !args.headless {
        if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))) {
            wb.set_look_path(dir.join("turbine/look.json"));
        }
    }
    if !args.headless {
        wb.start_extensions(turbine::workbench::extension_dirs());
    }
    // (Pictures where the terminal draws them: kitty's graphics protocol.)
    wb.set_images_inline(!args.headless && turbine_tui::graphics_supported());
    if let Some(k) = args.keymap {
        wb.set_keymap(k);
    }
    if let Some(s) = args.speed {
        wb.driver_mut().cps = s;
    } else if args.headless {
        wb.driver_mut().cps = 0.0;
    }

    let (tx, rx) = mpsc::channel::<Request>();
    wb.set_requests(rx);
    // The automation socket: agents (and `turbine mcp`) drive Turbine through it.
    let mut socket_path = None;
    let explicit = args.automation.is_some();
    if !args.no_automation && (explicit || !args.headless) {
        match server::listen(args.automation.clone().flatten(), tx.clone()) {
            Ok(p) => {
                if explicit {
                    eprintln!("turbine: automation socket {}", p.display());
                }
                wb.set_socket_path(p.clone());
                socket_path = Some(p);
            }
            Err(e) => eprintln!("turbine: automation socket failed: {e}"),
        }
    }
    if let Some(script) = &args.script {
        server::run_script(script.clone(), tx.clone());
    }
    if args.demo {
        wb.run_command("agent.demo");
    }
    // AI models: hearth's hub when it's installed (TURBINE_AI_HUB=0 skips it).
    if !args.headless || std::env::var("TURBINE_AI").is_ok() {
        wb.start_ai();
    }

    if args.headless {
        wb.event(Event::Resize(args.size.0, args.size.1));
        let script_done = server::script_done();
        let start = Instant::now();
        loop {
            let next = wb.tick(Instant::now());
            let script_idle = args.script.is_none() || script_done.load(std::sync::atomic::Ordering::SeqCst);
            let idle = script_idle && !wb.driver_mut().busy() && !wb.agent_running();
            if wb.should_quit() || idle || start.elapsed() > Duration::from_secs(600) {
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

    let r = turbine_tui::run(&mut wb);
    if let Some(p) = socket_path {
        let _ = std::fs::remove_file(p);
    }
    if let Err(e) = r {
        eprintln!("turbine: {e}");
        std::process::exit(1);
    }
}
