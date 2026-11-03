use std::env;
use std::io::{self, IsTerminal, Read, Write};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct Display {
    message: String,
    started: Instant,
    visible: bool,
}

impl Display {
    fn draw(&self, marker: &str, done: bool) {
        let mut stderr = io::stderr().lock();
        if done && !self.visible {
            let _ = writeln!(stderr);
        }
        let _ = write!(
            stderr,
            "\r\x1b[2K{marker} {}  {:.1}秒{}",
            self.message,
            self.started.elapsed().as_secs_f32(),
            if done { "\n" } else { "" }
        );
        let _ = stderr.flush();
    }
}

/// Owns one progress line. Dropping an unfinished stage marks it as failed.
pub struct Progress {
    display: Option<Arc<Mutex<Display>>>,
    worker: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
}

impl Progress {
    pub fn start(message: &str, plain: Option<&str>) -> Self {
        let enabled =
            io::stderr().is_terminal() && env::var_os("TERM").is_none_or(|term| term != "dumb");
        if !enabled {
            if let Some(plain) = plain {
                eprintln!("giclean: {plain}");
            }
            return Self {
                display: None,
                worker: None,
            };
        }
        let display = Arc::new(Mutex::new(Display {
            message: message.to_owned(),
            started: Instant::now(),
            visible: true,
        }));
        display.lock().unwrap().draw("⠋", false);
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
                    display.draw(frames[frame % frames.len()], false);
                    frame += 1;
                }
            }
        });
        Self {
            display: Some(display),
            worker: Some((send, worker)),
        }
    }

    pub fn update(&self, message: String) {
        if let Some(display) = &self.display {
            display.lock().unwrap().message = message;
        }
    }

    // Clear the progress line for stdout and child diagnostics.
    // Once another writer uses the terminal, stop redrawing for this stage.
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

    pub fn finish(mut self, message: &str, plain: Option<&str>) {
        self.stop();
        if let Some(display) = self.display.take() {
            let mut display = display.lock().unwrap();
            display.message = message.to_owned();
            display.draw("✔", true);
        } else if let Some(plain) = plain {
            eprintln!("giclean: {plain}");
        }
    }

    pub fn output(&self, command: &mut Command) -> io::Result<Output> {
        command.stdin(Stdio::null()).stdout(Stdio::piped());
        self.run(command, Child::wait_with_output)
    }

    pub fn status(&self, command: &mut Command) -> io::Result<ExitStatus> {
        self.run(command, |mut child| child.wait())
    }

    fn run<T>(
        &self,
        command: &mut Command,
        wait: impl FnOnce(Child) -> io::Result<T>,
    ) -> io::Result<T> {
        if self.display.is_none() {
            return wait(command.spawn()?);
        }
        let mut child = command.stderr(Stdio::piped()).spawn()?;
        let mut stderr = child.stderr.take().unwrap();
        thread::scope(|scope| {
            scope.spawn(|| {
                let mut buffer = [0; 4096];
                loop {
                    match stderr.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(size) => {
                            self.suspend();
                            let mut out = io::stderr().lock();
                            let _ = out.write_all(&buffer[..size]);
                            let _ = out.flush();
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
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
        if let Some(display) = &self.display {
            display.lock().unwrap().draw("✘", true);
        }
    }
}
