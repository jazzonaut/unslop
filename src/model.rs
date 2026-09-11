//! The llama-server sidecar.
//!
//! The server runs as a child process rather than being linked in, which keeps
//! clang, bindgen and the CUDA toolkit out of the build and means a crash in
//! the model takes the model down rather than the application.
//!
//! Only the process lifecycle lives here. Readiness is polled from the worker
//! thread, because loading 8B of weights takes seconds and the event loop must
//! stay responsive throughout.

use std::{
    io,
    net::TcpListener,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// Keeps a console window from flashing up behind the popup. Shared with the
/// installer, which also has to run a console application.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Bundled backends in preference order.
///
/// CUDA is an order of magnitude faster on NVIDIA hardware: measured on a 4070
/// the same rewrite takes 1.7s on CUDA and 20.6s on Vulkan, for identical VRAM
/// and identical output. Vulkan is kept because it is the only build that also
/// covers AMD and Intel, where it is the native path rather than a fallback.
const BACKENDS: [&str; 2] = [
    "runtime/llama-server.exe",
    "runtime-vulkan/llama-server.exe",
];

/// ponytail: a free port is claimed then released before the server binds it,
/// so another process could take it in between. Retry the range if that ever
/// actually happens.
const PORTS: std::ops::RangeInclusive<u16> = 8127..=8137;

struct Server {
    child: Child,
    port: u16,
    last_used: Instant,
}

pub struct Model {
    exe: PathBuf,
    weights: PathBuf,
    context: u32,
    gpu_layers: u32,
    /// 8GB of VRAM is not enough to hold the weights resident all day, so an
    /// unused server is shut down and started again on demand.
    idle_timeout: Duration,
    server: Option<Server>,
}

impl Model {
    pub fn new(
        exe: impl Into<PathBuf>,
        weights: impl Into<PathBuf>,
        idle_timeout: Duration,
        local: &crate::config::Local,
    ) -> Self {
        Self {
            exe: exe.into(),
            weights: weights.into(),
            context: local.context,
            gpu_layers: local.gpu_layers,
            idle_timeout,
            server: None,
        }
    }

    /// The port of a running server, starting one if needed.
    ///
    /// Returns as soon as the process exists; the weights are still loading.
    pub fn port(&mut self) -> io::Result<u16> {
        if let Some(server) = &mut self.server {
            // A server that died takes its port with it.
            if matches!(server.child.try_wait(), Ok(None)) {
                server.last_used = Instant::now();
                return Ok(server.port);
            }
            self.server = None;
        }

        let port = free_port()?;
        let child = Command::new(&self.exe)
            .arg("--model")
            .arg(&self.weights)
            .args(["--port", &port.to_string()])
            .args(["-ngl", &self.gpu_layers.to_string()])
            .args(["-c", &self.context.to_string()])
            // One slot: the default allocates four, for requests this
            // application never makes concurrently.
            .args(["-np", "1", "--no-webui"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;

        self.server = Some(Server {
            child,
            port,
            last_used: Instant::now(),
        });
        Ok(port)
    }

    /// Shut the server down if it has gone unused, releasing its VRAM.
    pub fn unload_if_idle(&mut self) {
        let idle = self
            .server
            .as_ref()
            .is_some_and(|server| server.last_used.elapsed() >= self.idle_timeout);
        if idle {
            self.stop();
        }
    }

    /// When the idle check next needs to run, for the event loop to wait on.
    pub fn idle_deadline(&self) -> Option<Instant> {
        let server = self.server.as_ref()?;
        Some(server.last_used + self.idle_timeout)
    }

    fn stop(&mut self) {
        if let Some(mut server) = self.server.take() {
            let _ = server.child.kill();
            // Reap it, or the weights stay in VRAM until the app exits.
            let _ = server.child.wait();
        }
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Whether the server on `port` has finished loading.
pub fn is_ready(port: u16) -> bool {
    ureq::get(format!("http://127.0.0.1:{port}/health"))
        .call()
        .is_ok_and(|response| response.status().is_success())
}

/// Block until the server answers, or give up.
///
/// Called from a worker thread: loading 8B of weights takes several seconds.
pub fn wait_until_ready(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if is_ready(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// A port nothing is listening on.
fn free_port() -> io::Result<u16> {
    PORTS
        .filter_map(|port| TcpListener::bind(("127.0.0.1", port)).ok())
        .map(|listener| listener.local_addr().map(|addr| addr.port()))
        .next()
        .unwrap_or_else(|| {
            Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "no free port in the range reserved for the model server",
            ))
        })
}

/// The fastest bundled backend that can actually see a device on this machine.
///
/// Each build is asked what it can see, which costs a few hundred milliseconds
/// once at startup and avoids shipping a CUDA-only binary to an AMD laptop.
/// `None` means no GPU backend works here, and the rules pass stands alone.
pub fn choose_backend() -> Option<PathBuf> {
    BACKENDS
        .iter()
        .map(|backend| crate::paths::beside_exe(backend))
        .find(|exe| exe.is_file() && sees_a_device(exe))
}

fn sees_a_device(exe: &Path) -> bool {
    let Ok(output) = Command::new(exe)
        .arg("--list-devices")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return false;
    };
    // Each device is listed on its own indented line, as `CUDA0: name` or
    // `Vulkan0: name`. A build with no usable device lists none.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.starts_with("  ") && line.contains(':'))
}

/// Ask the model to rewrite `text`, blocking until it answers.
///
/// Called from a worker thread. Thinking is disabled explicitly: Qwen3 turns
/// it on by default, which turns a two second rewrite into thirty seconds of
/// reasoning tokens nobody will read.
pub fn rewrite(
    port: u16,
    system: &str,
    text: &str,
    temperature: f32,
    max_tokens: u32,
) -> Result<String, String> {
    let request = serde_json::json!({
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": text},
        ],
        "chat_template_kwargs": {"enable_thinking": false},
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": false,
    });

    let mut response = ureq::post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .send_json(&request)
        .map_err(|err| format!("the model server did not answer: {err}"))?;

    let body: serde_json::Value = response
        .body_mut()
        .read_json()
        .map_err(|err| format!("the model server sent something unreadable: {err}"))?;

    body["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "the model returned no content".to_owned())
}
