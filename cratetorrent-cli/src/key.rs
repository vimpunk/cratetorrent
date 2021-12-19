use std::io;

use termion::{event::Key, input::TermRead};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver},
    task,
};

pub const EXIT_KEY: Key = Key::Char('q');

/// A small event handler that reads termion input events on a bocking tokio
/// task and forwards them via an mpsc channel to the main task. This can be
/// used to drive the application.
pub struct Keys {
    /// Input keys are sent to this channel.
    pub rx: UnboundedReceiver<Key>,
}

impl Keys {
    pub fn new(exit_key: Key) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        task::spawn(async move {
            let stdin = io::stdin();
            for key in stdin.keys().flatten() {
                if let Err(e) = tx.send(key) {
                    eprintln!("{}", e);
                    return;
                }
                if key == exit_key {
                    return;
                }
            }
        });
        Self { rx }
    }
}
