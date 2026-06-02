use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// A captured command string (the -c argument passed to bash/sh).
pub type Command = String;

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
                    let s = line.trim_end().to_string();
                    if !s.is_empty() {
                        commands.lock().unwrap().push(s);
                    }
                }
                Err(_) => break,
            }
        }
    });
}
