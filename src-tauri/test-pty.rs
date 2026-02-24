
use std::io::{Read, Write};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

fn main() {
    let pty_system = NativePtySystem::default();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }).unwrap();

    let mut cmd = CommandBuilder::new("cmd.exe");
    cmd.arg("/c");
    cmd.arg("gemini");
    cmd.arg("chat");

    let child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    std::thread::spawn(move || {
        let mut buf = [0; 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[0..n]).to_string();
                    print!("{}", text);
                    std::io::stdout().flush().unwrap();
                }
                Err(_) => break,
            }
        }
    });

    std::thread::sleep(std::time::Duration::from_millis(1000));
    writer.write_all(b"hello\r\n").unwrap();
    writer.flush().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5000));
    let _ = child;
}

