use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Command {
    pub id: u32,
    pub text: String,
    pub status: Option<i32>,
}

/// Tail `path` for new command lines as the wrapper writes them.
pub fn start_from_file(path: String, commands: Arc<Mutex<Vec<Command>>>) {
    thread::spawn(move || {
        while !Path::new(&path).exists() {
            thread::sleep(Duration::from_millis(30));
        }

        let file = File::open(&path).expect("failed to open trace file");
        let mut reader = BufReader::new(file);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => thread::sleep(Duration::from_millis(20)),
                Ok(_) => {
                    let s = line.trim_end();
                    if s.is_empty() {
                        continue;
                    }
                    if s.starts_with("START\t") {
                        let parts: Vec<&str> = s.splitn(3, '\t').collect();
                        if parts.len() == 3 {
                            if let Ok(id) = parts[1].parse::<u32>() {
                                let mut cmds = commands.lock().unwrap();
                                cmds.push(Command {
                                    id,
                                    text: parts[2].to_string(),
                                    status: None,
                                });
                            }
                        }
                    } else if s.starts_with("END\t") {
                        let parts: Vec<&str> = s.splitn(3, '\t').collect();
                        if parts.len() == 3 {
                            if let (Ok(id), Ok(status)) = (parts[1].parse::<u32>(), parts[2].parse::<i32>()) {
                                let mut cmds = commands.lock().unwrap();
                                // Update the last matching command (in case of PID reuse)
                                if let Some(cmd) = cmds.iter_mut().rev().find(|c| c.id == id) {
                                    cmd.status = Some(status);
                                }
                            }
                        }
                    } else {
                        // Fallback for old format or unparsed lines
                        let mut cmds = commands.lock().unwrap();
                        cmds.push(Command {
                            id: 0,
                            text: s.to_string(),
                            status: None,
                        });
                    }
                }
                Err(_) => break,
            }
        }
    });
}
