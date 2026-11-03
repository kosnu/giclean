use std::env;
use std::io::{self, IsTerminal, Read, Write};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

struct Display {
    message: String,
    visible: bool,
}

impl Display {
    fn draw(&self, marker: &str) {
        let mut stderr = io::stderr().lock();
        let _ = write!(stderr, "\r\x1b[2K{marker} {}", self.message);
        let _ = stderr.flush();
    }
}

/// Owns one transient progress line, cleared when processing ends.
pub struct Progress {
    display: Option<Arc<Mutex<Display>>>,
    worker: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
}

impl Progress {
    pub fn start(message: &str) -> Self {
        let enabled =
            io::stderr().is_terminal() && env::var_os("TERM").is_none_or(|term| term != "dumb");
        if !enabled {
            return Self {
                display: None,
                worker: None,
            };
        }
        let display = Arc::new(Mutex::new(Display {
            message: message.to_owned(),
            visible: true,
        }));
        display.lock().unwrap().draw("⠋");
        let state = Arc::clone(&display);
        let (send, receive) = mpsc::channel();
        let worker = thread::spawn(move || {
            let frames = ["⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "⠋"];
            let mut frame = 0;
            while matches!(
                receive.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                let display = state.lock().unwrap();
                if display.visible {
                    display.draw(frames[frame % frames.len()]);
                    frame += 1;
                }
            }
        });
        Self {
            display: Some(display),
            worker: Some((send, worker)),
        }
    }

    pub fn update(&self, message: &str) {
        if let Some(display) = &self.display {
            let mut display = display.lock().unwrap();
            display.message = message.to_owned();
            display.visible = true;
            display.draw("⠋");
        }
    }

    // Git, SSH, and credential helpers can write prompts directly to /dev/tty.
    // Stop drawing before starting any operation that can ask for input.
    pub fn pause_for_input(&self) {
        self.suspend();
    }

    // Clear the progress line for stdout and child diagnostics.
    // Drawing resumes only when the next progress message is set.
    pub fn suspend(&self) {
        if let Some(display) = &self.display {
            let mut display = display.lock().unwrap();
            if display.visible {
                let mut stderr = io::stderr().lock();
                let _ = write!(stderr, "\r\x1b[2K");
                let _ = stderr.flush();
                display.visible = false;
            }
        }
    }

    pub fn finish(mut self) {
        self.stop();
        self.suspend();
    }

    pub fn output(&self, command: &mut Command) -> io::Result<Output> {
        command.stdin(Stdio::null()).stdout(Stdio::piped());
        self.run(command, Child::wait_with_output)
    }

    pub fn output_with_input(&self, command: &mut Command, input: &[u8]) -> io::Result<Output> {
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        self.run(command, |mut child| {
            let mut stdin = child.stdin.take().unwrap();
            // Drain stdout/stderr while feeding stdin so large batches cannot
            // deadlock when both input and output exceed pipe capacity.
            thread::scope(|scope| {
                let writer = scope.spawn(move || stdin.write_all(input));
                let output = child.wait_with_output();
                writer.join().unwrap()?;
                output
            })
        })
    }

    fn run<T>(
        &self,
        command: &mut Command,
        wait: impl FnOnce(Child) -> io::Result<T>,
    ) -> io::Result<T> {
        if self.display.is_none() {
            return wait(command.spawn()?);
        }
        // Forward diagnostics even while drawing is paused for authentication,
        // so an unfinished line is terminated before any later redraw or error.
        let mut child = command.stderr(Stdio::piped()).spawn()?;
        let mut stderr = child.stderr.take().unwrap();
        thread::scope(|scope| {
            scope.spawn(|| {
                let mut buffer = [0; 4096];
                let mut needs_newline = false;
                loop {
                    match stderr.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(size) => {
                            self.suspend();
                            let mut out = io::stderr().lock();
                            let _ = out.write_all(&buffer[..size]);
                            let _ = out.flush();
                            needs_newline = buffer[size - 1] != b'\n';
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                if needs_newline {
                    eprintln!();
                }
            });
            wait(child)
        })
    }

    fn stop(&mut self) {
        if let Some((send, worker)) = self.worker.take() {
            let _ = send.send(());
            let _ = worker.join();
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.stop();
        self.suspend();
    }
}
