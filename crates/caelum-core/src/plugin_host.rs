//! Isolated-process host of the adapter plug-in SDK (F-ACQ-12; CR-026): local IPC with a child
//! process the core started after checking its hash against the signed manifest. It is not a
//! device link: the core owns every device connection, and the plug-in never gets one.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};

use caelum_devices::plugin::{
    from_wire, hex, AdapterPlugin, PluginError, WireReply, FRAME_TIME_LIMIT, MAX_FRAME_LEN,
    SDK_VERSION,
};
use caelum_devices::sig::Decoded;

/// An isolated-process plug-in. The executable is started only by the core after its hash was
/// checked against the signed manifest.
pub struct ProcessPlugin {
    id: String,
    program: std::path::PathBuf,
    running: Option<(Child, ChildStdin, Receiver<String>)>,
}

impl ProcessPlugin {
    /// A plug-in run from `program` (started on the first frame).
    pub fn new(id: &str, program: std::path::PathBuf) -> Self {
        Self {
            id: id.to_owned(),
            program,
            running: None,
        }
    }

    fn start(&mut self) -> Result<(), PluginError> {
        let mut child = Command::new(&self.program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| PluginError::Process(e.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PluginError::Process("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PluginError::Process("no stdout".into()))?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        self.running = Some((child, stdin, rx));
        Ok(())
    }

    fn stop(&mut self) {
        if let Some((mut child, _, _)) = self.running.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ProcessPlugin {
    fn drop(&mut self) {
        self.stop();
    }
}

impl AdapterPlugin for ProcessPlugin {
    fn id(&self) -> &str {
        &self.id
    }

    fn sdk_version(&self) -> u32 {
        SDK_VERSION
    }

    fn decode(&mut self, frame: &[u8]) -> Result<Vec<Decoded>, PluginError> {
        if frame.len() > MAX_FRAME_LEN {
            return Err(PluginError::FrameTooLong);
        }
        if self.running.is_none() {
            self.start()?;
        }
        let request = format!("{{\"sdk\":{SDK_VERSION},\"frame\":\"{}\"}}\n", hex(frame));
        let reply = {
            let Some((_, stdin, rx)) = self.running.as_mut() else {
                return Err(PluginError::Process("not running".into()));
            };
            if stdin
                .write_all(request.as_bytes())
                .and_then(|()| stdin.flush())
                .is_err()
            {
                None
            } else {
                rx.recv_timeout(FRAME_TIME_LIMIT).ok()
            }
        };
        let Some(line) = reply else {
            // Late, exited or broken: restart on the next frame.
            self.stop();
            return Err(PluginError::Process(
                "no reply within the time limit".into(),
            ));
        };
        let r: WireReply =
            serde_json::from_str(&line).map_err(|e| PluginError::Malformed(e.to_string()))?;
        match (r.readings, r.error) {
            (Some(rs), None) => rs.iter().map(from_wire).collect(),
            (None, Some(e)) => Err(PluginError::Refused(e)),
            _ => Err(PluginError::Malformed(
                "a reply has readings or an error".into(),
            )),
        }
    }
}
