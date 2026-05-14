use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_INTERVAL_SECS: u64 = 30;
const DEFAULT_PORT: u16 = 7878;
const LAUNCH_AGENT_LABEL: &str = "dev.local.wake-nudge";

fn main() {
    if let Err(err) = run() {
        eprintln!("wake-nudge: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let config = Config::parse(env::args().skip(1))?;

    if config.help {
        print_help();
        return Ok(());
    }

    if config.dry_run {
        println!(
            "dry run: would run the control button on http://127.0.0.1:{} and nudge once every {} seconds when enabled",
            config.port,
            config.interval.as_secs(),
        );
        if config.start_enabled {
            println!("dry run: simulation would start enabled");
        }
        return Ok(());
    }

    if let Some(command) = config.agent_command {
        return handle_agent_command(command, config);
    }

    if config.once {
        nudge_cursor()?;
        return Ok(());
    }

    run_controller(config)
}

#[derive(Debug, Clone, Copy)]
struct Config {
    interval: Duration,
    port: u16,
    agent_command: Option<AgentCommand>,
    start_enabled: bool,
    once: bool,
    dry_run: bool,
    help: bool,
}

#[derive(Debug, Clone, Copy)]
enum AgentCommand {
    Install,
    Uninstall,
    Start,
    Stop,
    Status,
}

impl Config {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, CliError> {
        let mut config = Self {
            interval: Duration::from_secs(DEFAULT_INTERVAL_SECS),
            port: DEFAULT_PORT,
            agent_command: None,
            start_enabled: false,
            once: false,
            dry_run: false,
            help: false,
        };

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => config.help = true,
                "--once" => config.once = true,
                "--dry-run" => config.dry_run = true,
                "--start-enabled" => config.start_enabled = true,
                "--install-agent" => config.set_agent_command(AgentCommand::Install)?,
                "--uninstall-agent" => config.set_agent_command(AgentCommand::Uninstall)?,
                "--start-agent" => config.set_agent_command(AgentCommand::Start)?,
                "--stop-agent" => config.set_agent_command(AgentCommand::Stop)?,
                "--agent-status" => config.set_agent_command(AgentCommand::Status)?,
                "-i" | "--interval" => {
                    let value = args
                        .next()
                        .ok_or_else(|| CliError::new("--interval requires a value in seconds"))?;
                    config.interval = parse_interval(&value)?;
                }
                "-p" | "--port" => {
                    let value = args
                        .next()
                        .ok_or_else(|| CliError::new("--port requires a value"))?;
                    config.port = parse_port(&value)?;
                }
                _ if arg.starts_with("--interval=") => {
                    let value = arg.trim_start_matches("--interval=");
                    config.interval = parse_interval(value)?;
                }
                _ if arg.starts_with("--port=") => {
                    let value = arg.trim_start_matches("--port=");
                    config.port = parse_port(value)?;
                }
                _ => return Err(CliError::new(format!("unknown argument: {arg}"))),
            }
        }

        Ok(config)
    }

    fn set_agent_command(&mut self, command: AgentCommand) -> Result<(), CliError> {
        if self.agent_command.is_some() {
            return Err(CliError::new(
                "only one agent command can be used at a time",
            ));
        }

        self.agent_command = Some(command);
        Ok(())
    }
}

fn parse_interval(value: &str) -> Result<Duration, CliError> {
    let seconds = value
        .parse::<u64>()
        .map_err(|_| CliError::new("interval must be a positive whole number of seconds"))?;

    if seconds == 0 {
        return Err(CliError::new("interval must be at least 1 second"));
    }

    Ok(Duration::from_secs(seconds))
}

fn parse_port(value: &str) -> Result<u16, CliError> {
    value
        .parse::<u16>()
        .map_err(|_| CliError::new("port must be a number from 0 to 65535"))
}

fn print_help() {
    println!(
        "\
wake-nudge

Runs a local Start/Stop button that controls tiny cursor nudges.

Usage:
  wake-nudge [options]

Options:
  -i, --interval <seconds>  Seconds between nudges (default: {DEFAULT_INTERVAL_SECS})
  -p, --port <port>         Local web control port (default: {DEFAULT_PORT})
      --once                Nudge once and exit
      --start-enabled       Start with simulation already on
      --dry-run             Print what would happen without moving the cursor
      --install-agent       Install and start a macOS LaunchAgent
      --uninstall-agent     Stop and remove the macOS LaunchAgent
      --start-agent         Start the installed macOS LaunchAgent
      --stop-agent          Stop the installed macOS LaunchAgent
      --agent-status        Print macOS LaunchAgent status
  -h, --help                Show this help
"
    );
}

#[derive(Debug)]
struct CliError {
    message: String,
}

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for CliError {}

fn handle_agent_command(command: AgentCommand, config: Config) -> Result<(), Box<dyn Error>> {
    match command {
        AgentCommand::Install => install_agent(config),
        AgentCommand::Uninstall => uninstall_agent(),
        AgentCommand::Start => start_agent(),
        AgentCommand::Stop => stop_agent(),
        AgentCommand::Status => status_agent(),
    }
}

#[cfg(target_os = "macos")]
fn install_agent(config: Config) -> Result<(), Box<dyn Error>> {
    let executable = env::current_exe()?;
    let plist_path = launch_agent_plist_path()?;
    let log_dir = home_path()?.join("Library").join("Logs").join("wake-nudge");

    fs::create_dir_all(
        plist_path
            .parent()
            .ok_or_else(|| CliError::new("LaunchAgents directory could not be resolved"))?,
    )?;
    fs::create_dir_all(&log_dir)?;
    fs::write(
        &plist_path,
        launch_agent_plist(
            &executable,
            config.interval.as_secs(),
            config.port,
            config.start_enabled,
            &log_dir,
        ),
    )?;

    let _ = run_launchctl(&[
        "bootout",
        &gui_domain()?,
        plist_path_string(&plist_path)?.as_str(),
    ]);
    run_launchctl(&[
        "bootstrap",
        &gui_domain()?,
        plist_path_string(&plist_path)?.as_str(),
    ])?;
    run_launchctl(&[
        "enable",
        &format!("{}/{}", gui_domain()?, LAUNCH_AGENT_LABEL),
    ])?;
    run_launchctl(&[
        "kickstart",
        "-k",
        &format!("{}/{}", gui_domain()?, LAUNCH_AGENT_LABEL),
    ])?;

    println!("installed LaunchAgent: {}", plist_path.display());
    println!("control button: http://127.0.0.1:{}", config.port);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn install_agent(_config: Config) -> Result<(), Box<dyn Error>> {
    Err("agent install is currently implemented only for macOS".into())
}

#[cfg(target_os = "macos")]
fn uninstall_agent() -> Result<(), Box<dyn Error>> {
    let plist_path = launch_agent_plist_path()?;
    let _ = run_launchctl(&[
        "bootout",
        &gui_domain()?,
        plist_path_string(&plist_path)?.as_str(),
    ]);

    if plist_path.exists() {
        fs::remove_file(&plist_path)?;
    }

    println!("removed LaunchAgent: {}", plist_path.display());
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn uninstall_agent() -> Result<(), Box<dyn Error>> {
    Err("agent uninstall is currently implemented only for macOS".into())
}

#[cfg(target_os = "macos")]
fn start_agent() -> Result<(), Box<dyn Error>> {
    run_launchctl(&[
        "kickstart",
        "-k",
        &format!("{}/{}", gui_domain()?, LAUNCH_AGENT_LABEL),
    ])?;
    println!("started LaunchAgent: {LAUNCH_AGENT_LABEL}");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn start_agent() -> Result<(), Box<dyn Error>> {
    Err("agent start is currently implemented only for macOS".into())
}

#[cfg(target_os = "macos")]
fn stop_agent() -> Result<(), Box<dyn Error>> {
    run_launchctl(&[
        "kill",
        "TERM",
        &format!("{}/{}", gui_domain()?, LAUNCH_AGENT_LABEL),
    ])?;
    println!("stopped LaunchAgent: {LAUNCH_AGENT_LABEL}");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn stop_agent() -> Result<(), Box<dyn Error>> {
    Err("agent stop is currently implemented only for macOS".into())
}

#[cfg(target_os = "macos")]
fn status_agent() -> Result<(), Box<dyn Error>> {
    run_launchctl_passthrough(&[
        "print",
        &format!("{}/{}", gui_domain()?, LAUNCH_AGENT_LABEL),
    ])
}

#[cfg(not(target_os = "macos"))]
fn status_agent() -> Result<(), Box<dyn Error>> {
    Err("agent status is currently implemented only for macOS".into())
}

#[cfg(target_os = "macos")]
fn launch_agent_plist_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(home_path()?
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCH_AGENT_LABEL}.plist")))
}

#[cfg(target_os = "macos")]
fn home_path() -> Result<PathBuf, Box<dyn Error>> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| CliError::new("HOME is not set").into())
}

#[cfg(target_os = "macos")]
fn gui_domain() -> Result<String, Box<dyn Error>> {
    Ok(format!("gui/{}", current_uid()?))
}

#[cfg(target_os = "macos")]
fn current_uid() -> Result<String, Box<dyn Error>> {
    let output = Command::new("id").arg("-u").output()?;
    if !output.status.success() {
        return Err("failed to read current user id".into());
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[cfg(target_os = "macos")]
fn plist_path_string(path: &PathBuf) -> Result<String, Box<dyn Error>> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| CliError::new("plist path is not valid UTF-8").into())
}

#[cfg(target_os = "macos")]
fn launch_agent_plist(
    executable: &PathBuf,
    interval_secs: u64,
    port: u16,
    start_enabled: bool,
    log_dir: &PathBuf,
) -> String {
    let executable = escape_xml(&executable.display().to_string());
    let stdout_log = escape_xml(&log_dir.join("stdout.log").display().to_string());
    let stderr_log = escape_xml(&log_dir.join("stderr.log").display().to_string());
    let start_enabled_args = if start_enabled {
        "    <string>--start-enabled</string>\n"
    } else {
        ""
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LAUNCH_AGENT_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{executable}</string>
    <string>--interval</string>
    <string>{interval_secs}</string>
    <string>--port</string>
    <string>{port}</string>
{start_enabled_args}
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{stdout_log}</string>
  <key>StandardErrorPath</key>
  <string>{stderr_log}</string>
</dict>
</plist>
"#
    )
}

#[cfg(target_os = "macos")]
fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "macos")]
fn run_launchctl(args: &[&str]) -> Result<(), Box<dyn Error>> {
    let status = Command::new("launchctl").args(args).status()?;
    if !status.success() {
        return Err(format!("launchctl {} failed", args.join(" ")).into());
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn run_launchctl_passthrough(args: &[&str]) -> Result<(), Box<dyn Error>> {
    let status = Command::new("launchctl").args(args).status()?;
    if !status.success() {
        return Err(format!("launchctl {} failed", args.join(" ")).into());
    }

    Ok(())
}

fn run_controller(config: Config) -> Result<(), Box<dyn Error>> {
    let enabled = Arc::new(AtomicBool::new(config.start_enabled));
    let worker_enabled = Arc::clone(&enabled);
    let interval = config.interval;

    thread::spawn(move || {
        let mut idle_guard: Option<IdleGuard> = None;
        let mut last_nudge: Option<Instant> = None;

        loop {
            let is_enabled = worker_enabled.load(Ordering::Relaxed);

            if is_enabled && idle_guard.is_none() {
                match prevent_idle_display_sleep() {
                    Ok(guard) => idle_guard = Some(guard),
                    Err(err) => {
                        eprintln!("wake-nudge: failed to prevent idle display sleep: {err}")
                    }
                }
            } else if !is_enabled {
                idle_guard = None;
                last_nudge = None;
            }

            let should_nudge =
                is_enabled && last_nudge.map_or(true, |last| last.elapsed() >= interval);

            if should_nudge {
                last_nudge = Some(Instant::now());
                if let Err(err) = nudge_cursor() {
                    eprintln!("wake-nudge: {err}");
                }
            }

            thread::sleep(Duration::from_secs(1));
        }
    });

    if config.start_enabled {
        println!("simulation starts enabled.");
    }

    let address = format!("127.0.0.1:{}", config.port);
    let listener = TcpListener::bind(&address)?;
    println!("wake-nudge control button: http://{address}");
    println!("Press Ctrl-C to quit.");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let enabled = Arc::clone(&enabled);
                thread::spawn(move || {
                    if let Err(err) = handle_connection(stream, enabled) {
                        eprintln!("wake-nudge: failed to handle request: {err}");
                    }
                });
            }
            Err(err) => eprintln!("wake-nudge: failed connection: {err}"),
        }
    }

    Ok(())
}

struct IdleGuard {
    #[cfg(target_os = "macos")]
    assertion_id: u32,
}

impl Drop for IdleGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        macos::release_idle_assertion(self.assertion_id);
    }
}

#[cfg(target_os = "macos")]
fn prevent_idle_display_sleep() -> Result<IdleGuard, Box<dyn Error>> {
    Ok(IdleGuard {
        assertion_id: macos::prevent_idle_display_sleep()?,
    })
}

#[cfg(not(target_os = "macos"))]
fn prevent_idle_display_sleep() -> Result<IdleGuard, Box<dyn Error>> {
    Err("idle display sleep prevention is currently implemented only for macOS".into())
}

fn handle_connection(
    mut stream: TcpStream,
    enabled: Arc<AtomicBool>,
) -> Result<(), Box<dyn Error>> {
    let mut buffer = [0; 2048];
    let bytes_read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let request_line = request.lines().next().unwrap_or_default();

    match request_line {
        "GET / HTTP/1.1" | "GET / HTTP/1.0" => {
            let body = page_html(enabled.load(Ordering::Relaxed));
            write_response(&mut stream, "200 OK", "text/html; charset=utf-8", &body)?;
        }
        "GET /status HTTP/1.1" | "GET /status HTTP/1.0" => {
            let body = status_json(enabled.load(Ordering::Relaxed));
            write_response(&mut stream, "200 OK", "application/json", &body)?;
        }
        "POST /toggle HTTP/1.1" | "POST /toggle HTTP/1.0" => {
            let new_state = !enabled.fetch_xor(true, Ordering::Relaxed);
            if new_state {
                if let Err(err) = nudge_cursor() {
                    enabled.store(false, Ordering::Relaxed);
                    let body = format!(r#"{{"enabled":false,"error":"{err}"}}"#);
                    write_response(
                        &mut stream,
                        "500 Internal Server Error",
                        "application/json",
                        &body,
                    )?;
                    return Ok(());
                }
            }
            let body = status_json(new_state);
            write_response(&mut stream, "200 OK", "application/json", &body)?;
        }
        _ => write_response(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found",
        )?,
    }

    Ok(())
}
fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<(), Box<dyn Error>> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()?;
    Ok(())
}

fn status_json(enabled: bool) -> String {
    format!(r#"{{"enabled":{enabled}}}"#)
}

fn page_html(enabled: bool) -> String {
    let initial_status = if enabled { "On" } else { "Off" };
    let initial_button = if enabled { "Stop" } else { "Start" };
    let initial_class = if enabled { "is-on" } else { "is-off" };

    format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>wake-nudge</title>
  <style>
    :root {{
      color-scheme: light dark;
      font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      background: #f6f4ef;
      color: #1e2421;
    }}
    body {{
      min-height: 100vh;
      margin: 0;
      display: grid;
      place-items: center;
    }}
    main {{
      width: min(420px, calc(100vw - 32px));
      display: grid;
      gap: 18px;
      text-align: center;
    }}
    h1 {{
      margin: 0;
      font-size: 2rem;
      font-weight: 720;
    }}
    p {{
      margin: 0;
      color: #58615c;
      line-height: 1.5;
    }}
    .status {{
      font-size: 1.1rem;
      font-weight: 700;
    }}
    .status.is-on {{
      color: #11694f;
    }}
    .status.is-off {{
      color: #8a3434;
    }}
    button {{
      min-height: 56px;
      border: 0;
      border-radius: 8px;
      background: #174f43;
      color: white;
      cursor: pointer;
      font-size: 1rem;
      font-weight: 750;
    }}
    button:hover {{
      background: #0f3f35;
    }}
    button:focus-visible {{
      outline: 3px solid #5aa892;
      outline-offset: 3px;
    }}
    @media (prefers-color-scheme: dark) {{
      :root {{
        background: #171917;
        color: #f1f0eb;
      }}
      p {{
        color: #b8bdb7;
      }}
      .status.is-on {{
        color: #75d6b4;
      }}
      .status.is-off {{
        color: #ef9b9b;
      }}
      button {{
        background: #3f8e78;
        color: #08110e;
      }}
      button:hover {{
        background: #55a78f;
      }}
    }}
  </style>
</head>
<body>
  <main>
    <h1>wake-nudge</h1>
    <p>Minimal local activity simulation is controlled from this button.</p>
    <div id="status" class="status {initial_class}">Simulation: {initial_status}</div>
    <button id="toggle" type="button">{initial_button}</button>
  </main>
  <script>
    const statusEl = document.querySelector("#status");
    const toggle = document.querySelector("#toggle");

    function render(enabled) {{
      statusEl.textContent = `Simulation: ${{enabled ? "On" : "Off"}}`;
      statusEl.className = `status ${{enabled ? "is-on" : "is-off"}}`;
      toggle.textContent = enabled ? "Stop" : "Start";
    }}

    async function refresh() {{
      const response = await fetch("/status");
      render((await response.json()).enabled);
    }}

    toggle.addEventListener("click", async () => {{
      toggle.disabled = true;
      try {{
        const response = await fetch("/toggle", {{ method: "POST" }});
        render((await response.json()).enabled);
      }} finally {{
        toggle.disabled = false;
      }}
    }});

    setInterval(refresh, 5000);
  </script>
</body>
</html>"##
    )
}

#[cfg(target_os = "macos")]
fn nudge_cursor() -> Result<(), Box<dyn Error>> {
    macos::nudge_cursor();
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn nudge_cursor() -> Result<(), Box<dyn Error>> {
    Err("cursor nudging is currently implemented only for macOS".into())
}

#[cfg(target_os = "macos")]
mod macos {
    use std::error::Error;
    use std::ffi::{c_char, c_void, CString};

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;
    const IOPM_ASSERTION_TYPE_PREVENT_USER_IDLE_DISPLAY_SLEEP: &str = "PreventUserIdleDisplaySleep";
    const IOPM_ASSERTION_NAME: &str = "wake-nudge active";

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    type CGEventRef = *mut c_void;
    type CFStringRef = *const c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const c_void) -> CGEventRef;
        fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
        fn CGWarpMouseCursorPosition(new_cursor_position: CGPoint) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            assertion_level: u32,
            assertion_name: CFStringRef,
            assertion_id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(assertion_id: u32) -> i32;
    }

    pub fn nudge_cursor() {
        unsafe {
            let event = CGEventCreate(std::ptr::null());
            if event.is_null() {
                return;
            }

            let location = CGEventGetLocation(event);
            CFRelease(event.cast_const());

            let right = CGPoint {
                x: location.x + 1.0,
                y: location.y,
            };
            let _ = CGWarpMouseCursorPosition(right);
            let _ = CGWarpMouseCursorPosition(location);
        }
    }

    pub fn prevent_idle_display_sleep() -> Result<u32, Box<dyn Error>> {
        unsafe {
            let assertion_type = cf_string(IOPM_ASSERTION_TYPE_PREVENT_USER_IDLE_DISPLAY_SLEEP)?;
            let assertion_name = cf_string(IOPM_ASSERTION_NAME)?;
            let mut assertion_id = 0;
            let result = IOPMAssertionCreateWithName(
                assertion_type,
                K_IOPM_ASSERTION_LEVEL_ON,
                assertion_name,
                &mut assertion_id,
            );

            CFRelease(assertion_type);
            CFRelease(assertion_name);

            if result != 0 {
                return Err(format!("IOPMAssertionCreateWithName returned {result}").into());
            }

            Ok(assertion_id)
        }
    }

    pub fn release_idle_assertion(assertion_id: u32) {
        unsafe {
            let _ = IOPMAssertionRelease(assertion_id);
        }
    }

    unsafe fn cf_string(value: &str) -> Result<CFStringRef, Box<dyn Error>> {
        let value = CString::new(value)?;
        let string =
            CFStringCreateWithCString(std::ptr::null(), value.as_ptr(), K_CF_STRING_ENCODING_UTF8);

        if string.is_null() {
            return Err("CFStringCreateWithCString returned null".into());
        }

        Ok(string)
    }
}
