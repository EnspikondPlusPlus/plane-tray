#![windows_subsystem = "windows"]

use std::net::{SocketAddr, TcpStream};
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use tao::event_loop::{ControlFlow, EventLoop};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

const WSL_DISTRO: &str = "Ubuntu-24.04";
const PLANECTL_PATH: &str = "/home/ivanz/plane/planectl.sh";
const FRONTEND_PORT: u16 = 18080;
const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// Prevents a console window from flashing up for each spawned child process.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const RED_ICO: &[u8] = include_bytes!("icons/rp.ico");
const YELLOW_ICO: &[u8] = include_bytes!("icons/yp.ico");
const GREEN_ICO: &[u8] = include_bytes!("icons/gp.ico");

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlaneStatus {
    Stopped,
    Starting,
    Running,
}

fn run_setup_command(cmd: &str) {
    let shell_cmd = format!("{} {}", PLANECTL_PATH, cmd);
    Command::new("wsl.exe")
        .args(["-d", WSL_DISTRO, "--", "bash", "-lc", &shell_cmd])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("[ERROR] failed to run planectl.sh");
}

fn start_plane() {
    run_setup_command("start");
}

fn stop_plane() {
    run_setup_command("stop");
}

fn restart_plane() {
    run_setup_command("restart");
}

fn open_plane() {
    Command::new("cmd")
        .args(["/C", "start", &format!("http://localhost:{}", FRONTEND_PORT)])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
}

/// Plane runs as a set of docker containers using `makeplane/...` images.
/// We list running containers' images and look for that prefix ourselves,
/// rather than relying on a docker-side filter, to avoid filter/escaping issues.
fn is_docker_up() -> bool {
    let output = Command::new("wsl.exe")
        .args([
            "-d",
            WSL_DISTRO,
            "--",
            "bash",
            "-lc",
            "docker ps --format '{{.Image}}'",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    match output {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|line| line.contains("makeplane")),
        Err(_) => false,
    }
}

/// Checks whether the Plane frontend is accepting connections on localhost.
fn is_frontend_up() -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], FRONTEND_PORT).into();
    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

fn plane_status() -> PlaneStatus {
    if !is_docker_up() {
        return PlaneStatus::Stopped;
    }
    if is_frontend_up() {
        PlaneStatus::Running
    } else {
        PlaneStatus::Starting
    }
}

/// Spawns a background thread that periodically checks Plane's status
/// and reports it back over a channel.
fn spawn_status_poller() -> Receiver<PlaneStatus> {
    let (tx, rx) = channel::<PlaneStatus>();
    std::thread::spawn(move || loop {
        if tx.send(plane_status()).is_err() {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    });
    rx
}

/// Writes an embedded .ico's bytes to a stable temp file (once) and loads it
/// as a tray Icon. tray-icon's Windows backend needs an on-disk path to load from.
fn load_icon(bytes: &[u8], name: &str) -> Icon {
    let path = std::env::temp_dir().join(format!("plane-tray-{name}.ico"));
    if !path.exists() {
        std::fs::write(&path, bytes).expect("failed to write tray icon to temp dir");
    }
    Icon::from_path(&path, None).expect("failed to load tray icon")
}

struct StatusIcons {
    red: Icon,
    yellow: Icon,
    green: Icon,
}

fn apply_status(
    tray: &TrayIcon,
    icons: &StatusIcons,
    status_item: &MenuItem,
    start: &MenuItem,
    stop: &MenuItem,
    status: PlaneStatus,
) {
    let (text, icon) = match status {
        PlaneStatus::Stopped => ("Status: ○ Stopped", &icons.red),
        PlaneStatus::Starting => ("Status: ◐ Starting up...", &icons.yellow),
        PlaneStatus::Running => ("Status: ● Running", &icons.green),
    };
    status_item.set_text(text);
    tray.set_icon(Some(icon.clone())).ok();
    start.set_enabled(status == PlaneStatus::Stopped);
    stop.set_enabled(status != PlaneStatus::Stopped);
}

fn main() {
    let event_loop = EventLoop::new();

    let icons = StatusIcons {
        red: load_icon(RED_ICO, "red"),
        yellow: load_icon(YELLOW_ICO, "yellow"),
        green: load_icon(GREEN_ICO, "green"),
    };

    // ----- MENU ITEMS -----
    let status_item = MenuItem::with_id("status", "Status: checking...", false, None);
    let start = MenuItem::with_id("start", "Start Plane", false, None);
    let stop = MenuItem::with_id("stop", "Stop Plane", false, None);
    let restart = MenuItem::with_id("restart", "Restart Plane", true, None);
    let open = MenuItem::with_id("open", "Open Plane", true, None);
    let quit = MenuItem::with_id("quit", "Exit", true, None);

    let menu = Menu::new();
    menu.append(&status_item).unwrap();
    menu.append(&PredefinedMenuItem::separator()).unwrap();
    menu.append(&start).unwrap();
    menu.append(&stop).unwrap();
    menu.append(&restart).unwrap();
    menu.append(&open).unwrap();
    menu.append(&PredefinedMenuItem::separator()).unwrap();
    menu.append(&quit).unwrap();

    // ----- TRAY ICON -----
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Plane")
        .with_icon(icons.red.clone())
        .build()
        .unwrap();

    let status_rx = spawn_status_poller();

    // ----- EVENT LOOP -----
    event_loop.run(move |_event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(250));

        while let Ok(status) = status_rx.try_recv() {
            apply_status(&tray, &icons, &status_item, &start, &stop, status);
        }

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            match event.id.as_ref() {
                "start" => start_plane(),
                "stop" => stop_plane(),
                "restart" => restart_plane(),
                "open" => open_plane(),
                "quit" => std::process::exit(0),
                _ => {}
            }
        }
    });
}
