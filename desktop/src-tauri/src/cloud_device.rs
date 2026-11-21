use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::Duration,
};
use tauri::{Manager, State, WebviewUrl, WebviewWindowBuilder};

struct Executor {
    generation: u64,
    child: Option<Child>,
    status: &'static str,
}

impl Default for Executor {
    fn default() -> Self {
        Self {
            generation: 0,
            child: None,
            status: "offline",
        }
    }
}

#[derive(Default)]
struct Device(Mutex<Executor>);

#[tauri::command]
fn device_status(device: State<'_, Device>) -> String {
    device.0.lock().expect("device state lock").status.into()
}

#[tauri::command]
fn disconnect(device: State<'_, Device>) {
    let child = {
        let mut active = device.0.lock().expect("device state lock");
        active.generation += 1;
        active.status = "offline";
        active.child.take()
    };
    if let Some(mut child) = child {
        #[cfg(unix)]
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGTERM)
        };
        std::thread::spawn(move || {
            for _ in 0..25 {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            let _ = child.kill();
            let _ = child.wait();
        });
    }
}

#[tauri::command]
fn connect(app: tauri::AppHandle, device: State<'_, Device>) -> Result<(), String> {
    let origin = option_env!("JEEVES_PUBLIC_ORIGIN")
        .filter(|origin| origin.starts_with("https://"))
        .ok_or("This build needs the Jeeves service origin")?;
    let binary = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = binary
        .parent()
        .ok_or("Device binaries were not installed")?;
    let mut active = device.0.lock().expect("device state lock");
    if active.child.is_some() {
        return Ok(());
    }
    let mut child = Command::new(dir.join("jeeves-device"))
        .arg("mac-device")
        .arg(origin)
        .arg(dir.join("jeeves-window"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| e.to_string())?;
    let pipe = child.stdout.take().ok_or("No device status output")?;
    active.generation += 1;
    let generation = active.generation;
    active.status = "connecting";
    active.child = Some(child);
    drop(active);
    std::thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            let status = match line.as_str() {
                "pairing" => "pairing",
                "online" => "online",
                "executing" => "executing",
                "offline" => "reconnecting",
                _ => continue,
            };
            let device = app.state::<Device>();
            let mut active = device.0.lock().expect("device state lock");
            if active.generation != generation {
                return;
            }
            active.status = status;
        }
        let device = app.state::<Device>();
        let mut active = device.0.lock().expect("device state lock");
        if active.generation == generation {
            if let Some(mut child) = active.child.take() {
                let _ = child.wait();
            }
            active.status = "offline";
        }
    });
    Ok(())
}

pub(super) fn main() {
    tauri::Builder::default()
        .manage(Device::default())
        .invoke_handler(tauri::generate_handler![connect, disconnect, device_status])
        .setup(|app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("device.html".into()))
                .title("Jeeves")
                .inner_size(420.0, 290.0)
                .resizable(false)
                .build()?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Jeeves device application could not start")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                disconnect(app.state());
            }
        });
}
